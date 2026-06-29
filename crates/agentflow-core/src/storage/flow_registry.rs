use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::domain::StepStatus;

use super::module_registry::ModuleSpec;
use super::project_store::{now_unix_seconds, EventRecord, ProjectStore, StorageError};
use super::tool_registry::{validate_param_value, ExecutableToolSpec};
use super::yaml;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowDraft {
    pub schema_version: String,
    pub id: String,
    pub name: String,
    pub steps: Vec<FlowStepDraft>,
    pub source_text: String,
}

impl FlowDraft {
    /// Parse a flow draft. Any step that references a `module:` is rejected
    /// (use [`FlowDraft::from_simple_yaml_with_modules`] to supply the modules).
    pub fn from_simple_yaml(source_text: &str) -> Result<Self, StorageError> {
        Self::from_simple_yaml_with_modules(source_text, &BTreeMap::new())
    }

    /// Parse a flow draft, inline-expanding any `module: <ref>` step using the
    /// supplied modules (keyed by `namespace/name`). After expansion the returned
    /// draft contains only tool steps, so the existing scheduler/runtime run it
    /// unchanged.
    pub fn from_simple_yaml_with_modules(
        source_text: &str,
        modules: &BTreeMap<String, ModuleSpec>,
    ) -> Result<Self, StorageError> {
        let raw = yaml::parse_yaml::<RawFlowDraft>("flow", source_text)?;
        let schema_version =
            required_flow_field(raw.schema_version.clone(), "schema_version", source_text)?;
        if schema_version != agentflow_schemas::FLOW_SCHEMA_V0 {
            return Err(yaml::invalid_input_at_field(
                source_text,
                "schema_version",
                format!(
                    "flow schema_version must be {}",
                    agentflow_schemas::FLOW_SCHEMA_V0
                ),
            ));
        }

        let id = required_flow_field(raw.id.clone(), "id", source_text)?;
        let name = required_flow_field(raw.name.clone(), "name", source_text)?;
        validate_ref_part("flow id", &id)
            .map_err(|error| yaml::with_field_location(source_text, "id", error))?;
        if name.trim().is_empty() {
            return Err(yaml::invalid_input_at_field(
                source_text,
                "name",
                "flow name must not be empty".to_string(),
            ));
        }

        Ok(Self {
            schema_version,
            id,
            name,
            steps: resolve_flow_steps(raw.steps, modules, source_text)?,
            source_text: source_text.to_string(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowStepDraft {
    pub id: String,
    pub tool_ref: String,
    pub needs: Vec<String>,
    pub reason: Option<String>,
    pub inputs: BTreeMap<String, String>,
    pub params: BTreeMap<String, String>,
    pub outputs: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct RawFlowDraft {
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    schema_version: Option<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    id: Option<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    name: Option<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_default_vec")]
    steps: Vec<RawFlowStepDraft>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFlowStepDraft {
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    id: Option<String>,
    #[serde(
        rename = "tool",
        default,
        deserialize_with = "yaml::deserialize_optional_scalar_string"
    )]
    tool_ref: Option<String>,
    #[serde(
        rename = "module",
        default,
        deserialize_with = "yaml::deserialize_optional_scalar_string"
    )]
    module_ref: Option<String>,
    #[serde(
        rename = "type",
        default,
        deserialize_with = "yaml::deserialize_optional_scalar_string"
    )]
    _step_type: Option<String>,
    #[serde(
        default,
        deserialize_with = "yaml::deserialize_optional_present_scalar_string"
    )]
    reason: Option<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_string_vec_or_csv")]
    needs: Vec<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_string_map")]
    inputs: BTreeMap<String, String>,
    #[serde(default, deserialize_with = "yaml::deserialize_string_map")]
    params: BTreeMap<String, String>,
    #[serde(default, deserialize_with = "yaml::deserialize_string_map")]
    outputs: BTreeMap<String, String>,
}

impl RawFlowStepDraft {
    fn into_step(self, source_text: &str) -> Result<FlowStepDraft, StorageError> {
        let step = FlowStepDraft {
            id: self.id.unwrap_or_default(),
            tool_ref: self.tool_ref.unwrap_or_default(),
            needs: self.needs,
            reason: self.reason,
            inputs: self.inputs,
            params: self.params,
            outputs: self.outputs,
        };
        finalize_raw_step(step, source_text)
    }
}

fn finalize_raw_step(
    step: FlowStepDraft,
    source_text: &str,
) -> Result<FlowStepDraft, StorageError> {
    if step.id.trim().is_empty() {
        return Err(yaml::invalid_input_at_field(
            source_text,
            "id",
            "flow step is missing id",
        ));
    }
    Ok(step)
}

/// Per module-instance bookkeeping needed to rewire the rest of the flow.
struct ModuleInstance {
    /// External output port -> (exposed instance-prefixed artifact name, the
    /// inlined step that produces it). Kept as a pair so a consumer rewrite can
    /// never resolve the artifact without also adding the producer `needs` edge.
    outputs: BTreeMap<String, (String, String)>,
    /// Steps a `needs: [instance]` ordering edge should point at (the producers
    /// of the exposed outputs, or every inlined step if nothing is exposed).
    sink_step_ids: Vec<String>,
}

/// Resolve raw flow steps into tool-only [`FlowStepDraft`]s, inline-expanding any
/// `module:` step and rewiring cross-instance `needs`/output references so the
/// flattened flow runs on the existing scheduler unchanged.
fn resolve_flow_steps(
    raw_steps: Vec<RawFlowStepDraft>,
    modules: &BTreeMap<String, ModuleSpec>,
    source_text: &str,
) -> Result<Vec<FlowStepDraft>, StorageError> {
    let mut steps: Vec<FlowStepDraft> = Vec::new();
    let mut instances: BTreeMap<String, ModuleInstance> = BTreeMap::new();

    for raw in raw_steps {
        let id = raw.id.clone().unwrap_or_default();
        if id.trim().is_empty() {
            return Err(yaml::invalid_input_at_field(
                source_text,
                "id",
                "flow step is missing id",
            ));
        }
        let has_tool = raw
            .tool_ref
            .as_deref()
            .is_some_and(|t| !t.trim().is_empty());
        let module_ref = raw
            .module_ref
            .as_deref()
            .filter(|m| !m.trim().is_empty())
            .map(str::to_string);

        let Some(module_ref) = module_ref else {
            // Ordinary tool step (or a tool-less step the flow validator rejects).
            steps.push(raw.into_step(source_text)?);
            continue;
        };
        if has_tool {
            return Err(StorageError::InvalidInput(format!(
                "flow step {id} declares both tool and module"
            )));
        }
        // A module instance id is the `head` of `instance.port` input references,
        // which are split on the first '.', so it must not contain one.
        if id.contains('.') {
            return Err(StorageError::InvalidInput(format!(
                "module step id {id} must not contain '.' (reserved for instance.port references)"
            )));
        }

        let spec = modules.get(&module_ref).ok_or_else(|| {
            StorageError::InvalidInput(format!(
                "flow step {id} references module {module_ref}, which was not provided"
            ))
        })?;
        // The module step's `inputs` are the bindings for the module's input
        // ports; its `needs` become the upstream dependency of the instance's
        // source steps.
        let expansion = spec.expand(&id, &raw.inputs)?;
        let module_needs = raw.needs.clone();

        // Map each inlined artifact name to its producing step (artifact names are
        // unique within an instance — guaranteed by ModuleSpec::validate).
        let mut artifact_producer: BTreeMap<&str, &str> = BTreeMap::new();
        for inlined in &expansion.steps {
            for artifact in inlined.outputs.values() {
                artifact_producer.insert(artifact.as_str(), inlined.id.as_str());
            }
        }
        // Pair each exposed output port with (artifact, producer step) together so
        // a downstream rewrite always gets both — they can never decouple.
        let mut outputs: BTreeMap<String, (String, String)> = BTreeMap::new();
        for (port, artifact) in &expansion.outputs {
            let producer = artifact_producer.get(artifact.as_str()).ok_or_else(|| {
                StorageError::InvalidInput(format!(
                    "module {module_ref} output port {port} maps to {artifact}, which no \
                     inlined step produces"
                ))
            })?;
            outputs.insert(port.clone(), (artifact.clone(), (*producer).to_string()));
        }
        let mut sink_step_ids: Vec<String> = outputs
            .values()
            .map(|(_, producer)| producer.clone())
            .collect();
        dedupe_in_place(&mut sink_step_ids);
        if sink_step_ids.is_empty() {
            // A module with no exposed outputs: a `needs: [instance]` edge orders
            // after the whole module, so depend on every inlined step.
            sink_step_ids = expansion.steps.iter().map(|s| s.id.clone()).collect();
        }

        for mut inlined in expansion.steps {
            // Source-step detection relies on ModuleSpec::validate() requiring every
            // internal artifact consumer to declare needs on its producer: after
            // expand(), `needs.is_empty()` iff the step is a topological source.
            // Such sources inherit the module step's upstream needs; deeper steps
            // already depend through them.
            if inlined.needs.is_empty() && !module_needs.is_empty() {
                inlined.needs = module_needs.clone();
            }
            steps.push(inlined);
        }
        instances.insert(
            id,
            ModuleInstance {
                outputs,
                sink_step_ids,
            },
        );
    }

    // Second pass: rewire references to module instances across every step.
    for step in &mut steps {
        let mut needs: Vec<String> = Vec::new();
        for need in std::mem::take(&mut step.needs) {
            match instances.get(&need) {
                Some(instance) => needs.extend(instance.sink_step_ids.iter().cloned()),
                None => needs.push(need),
            }
        }
        for value in step.inputs.values_mut() {
            let Some((head, port)) = value.split_once('.') else {
                continue;
            };
            let Some(instance) = instances.get(head) else {
                continue;
            };
            let (artifact, producer) = instance.outputs.get(port).ok_or_else(|| {
                StorageError::InvalidInput(format!(
                    "flow step {} input references {head}.{port}, but module instance {head} \
                     has no output port {port}",
                    step.id
                ))
            })?;
            *value = artifact.clone();
            needs.push(producer.clone());
        }
        dedupe_in_place(&mut needs);
        step.needs = needs;
    }

    Ok(steps)
}

/// Remove duplicates while preserving first-seen order.
fn dedupe_in_place(items: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    items.retain(|item| seen.insert(item.clone()));
}

fn required_flow_field(
    value: Option<String>,
    field_name: &str,
    source_text: &str,
) -> Result<String, StorageError> {
    value.filter(|value| !value.is_empty()).ok_or_else(|| {
        yaml::invalid_input_at_field(
            source_text,
            field_name,
            format!("flow spec is missing required field {field_name}"),
        )
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowValidationIssue {
    pub severity: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowValidationReport {
    pub flow_id: String,
    pub name: String,
    pub valid: bool,
    pub step_count: usize,
    pub edge_count: usize,
    pub issues: Vec<FlowValidationIssue>,
}

impl FlowValidationReport {
    pub fn to_json(&self) -> String {
        serde_json::to_string(&FlowValidationReportJson {
            schema_version: agentflow_schemas::FLOW_VALIDATION_JSON_SCHEMA_V0.to_string(),
            flow_id: self.flow_id.clone(),
            name: self.name.clone(),
            valid: self.valid,
            step_count: self.step_count,
            edge_count: self.edge_count,
            issues: self
                .issues
                .iter()
                .map(|issue| FlowValidationIssueJson {
                    severity: issue.severity.clone(),
                    message: issue.message.clone(),
                })
                .collect(),
        })
        .expect("flow validation report serializes to JSON")
    }

    fn error_message(&self) -> String {
        self.issues
            .iter()
            .map(|issue| issue.message.clone())
            .collect::<Vec<_>>()
            .join("; ")
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct FlowValidationReportJson {
    schema_version: String,
    flow_id: String,
    name: String,
    valid: bool,
    step_count: usize,
    edge_count: usize,
    issues: Vec<FlowValidationIssueJson>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FlowValidationIssueJson {
    severity: String,
    message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowApproval {
    pub flow_id: String,
    pub name: String,
    pub step_count: usize,
    pub edge_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowInspection {
    pub id: String,
    pub name: String,
    pub status: String,
    pub source_path: Option<PathBuf>,
    pub schema_version: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub steps: Vec<StoredFlowStep>,
    pub edges: Vec<StoredFlowEdge>,
}

impl FlowInspection {
    pub fn to_json(&self) -> String {
        serde_json::to_string(&FlowInspectionJson {
            schema_version: agentflow_schemas::FLOW_INSPECTION_JSON_SCHEMA_V0.to_string(),
            flow: FlowInspectionFlowJson {
                id: self.id.clone(),
                name: self.name.clone(),
                status: self.status.clone(),
                source_path: self
                    .source_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
                flow_schema_version: self.schema_version.clone(),
                created_at: self.created_at,
                updated_at: self.updated_at,
                steps: self.steps.iter().map(flow_step_json).collect(),
                edges: self
                    .edges
                    .iter()
                    .map(|edge| FlowInspectionEdgeJson {
                        from: edge.from_local_id.clone(),
                        to: edge.to_local_id.clone(),
                        edge_type: edge.edge_type.clone(),
                    })
                    .collect(),
            },
        })
        .expect("flow inspection serializes to JSON")
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct FlowInspectionJson {
    schema_version: String,
    flow: FlowInspectionFlowJson,
}

#[derive(Debug, Serialize, Deserialize)]
struct FlowInspectionFlowJson {
    id: String,
    name: String,
    status: String,
    source_path: Option<String>,
    flow_schema_version: String,
    created_at: i64,
    updated_at: i64,
    steps: Vec<FlowInspectionStepJson>,
    edges: Vec<FlowInspectionEdgeJson>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FlowInspectionStepJson {
    id: String,
    local_id: String,
    tool_ref: Option<String>,
    #[serde(rename = "type")]
    step_type: String,
    status: String,
    reason: Option<String>,
    inputs: serde_json::Value,
    params: serde_json::Value,
    outputs: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct FlowInspectionEdgeJson {
    from: String,
    to: String,
    #[serde(rename = "type")]
    edge_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFlowStep {
    pub id: String,
    pub local_id: String,
    pub tool_ref: Option<String>,
    pub step_type: String,
    pub status: String,
    pub reason: Option<String>,
    pub params_json: String,
    pub inputs_json: String,
    pub outputs_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFlowEdge {
    pub from_step_id: String,
    pub to_step_id: String,
    pub from_local_id: String,
    pub to_local_id: String,
    pub edge_type: String,
}

impl ProjectStore {
    pub fn validate_flow(&self, draft: &FlowDraft) -> FlowValidationReport {
        let mut issues = Vec::new();
        let mut ids = BTreeSet::new();
        let output_names_by_step = draft
            .steps
            .iter()
            .map(|step| {
                (
                    step.id.clone(),
                    step.outputs.keys().cloned().collect::<BTreeSet<_>>(),
                )
            })
            .collect::<BTreeMap<_, _>>();

        if draft.steps.is_empty() {
            issues.push(issue("flow must contain at least one step"));
        }

        for step in &draft.steps {
            if step.id.trim().is_empty() {
                issues.push(issue("step id must not be empty"));
                continue;
            }
            if validate_ref_part("step id", &step.id).is_err() {
                issues.push(issue(format!(
                    "step id {} contains invalid characters",
                    step.id
                )));
            }
            if !ids.insert(step.id.clone()) {
                issues.push(issue(format!("duplicate step id {}", step.id)));
            }
        }

        for step in &draft.steps {
            if step.tool_ref.trim().is_empty() {
                issues.push(issue(format!("step {} is missing tool", step.id)));
            } else {
                match self.executable_tool(&step.tool_ref) {
                    Ok(tool) => validate_step_against_tool(step, &tool, &mut issues),
                    Err(error) => {
                        issues.push(issue(format!(
                            "step {} references unavailable or non-executable tool {}: {}",
                            step.id, step.tool_ref, error
                        )));
                    }
                }
            }

            for need in &step.needs {
                if need == &step.id {
                    issues.push(issue(format!("step {} cannot need itself", step.id)));
                } else if !ids.contains(need) {
                    issues.push(issue(format!(
                        "step {} needs unknown step {}",
                        step.id, need
                    )));
                }
            }

            for (input_name, input_value) in &step.inputs {
                validate_input_reference(
                    self,
                    &ids,
                    &step.id,
                    input_name,
                    input_value,
                    &output_names_by_step,
                    &mut issues,
                );
            }
        }

        if has_cycle(&draft.steps) {
            issues.push(issue("flow contains a dependency cycle"));
        }

        FlowValidationReport {
            flow_id: draft.id.clone(),
            name: draft.name.clone(),
            valid: issues.is_empty(),
            step_count: draft.steps.len(),
            edge_count: draft.steps.iter().map(|step| step.needs.len()).sum(),
            issues,
        }
    }

    pub fn approve_flow(
        &self,
        draft: FlowDraft,
        source_path: Option<&Path>,
    ) -> Result<FlowApproval, StorageError> {
        let report = self.validate_flow(&draft);
        if !report.valid {
            return Err(StorageError::InvalidInput(format!(
                "flow validation failed: {}",
                report.error_message()
            )));
        }

        let exists: Option<String> = self
            .connection()
            .query_row(
                "SELECT id FROM flows WHERE id = ?1",
                params![&draft.id],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_some() {
            return Err(StorageError::InvalidInput(format!(
                "flow {} is already approved",
                draft.id
            )));
        }

        let now = now_unix_seconds();
        let source_path = source_path.map(|path| path.display().to_string());
        self.connection().execute(
            "INSERT INTO flows (id, name, status, source_path, schema_version, created_at, updated_at)
             VALUES (?1, ?2, 'approved', ?3, ?4, ?5, ?6)",
            params![
                &draft.id,
                &draft.name,
                source_path,
                &draft.schema_version,
                now,
                now
            ],
        )?;

        for step in &draft.steps {
            self.connection().execute(
                "INSERT INTO steps
                 (id, flow_id, tool_ref, type, status, reason, params_json, inputs_json, outputs_json, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'analysis', ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    db_step_id(&draft.id, &step.id),
                    &draft.id,
                    &step.tool_ref,
                    StepStatus::Draft.as_str(),
                    &step.reason,
                    map_json(&step.params),
                    map_json(&step.inputs),
                    map_json(&step.outputs),
                    now,
                    now
                ],
            )?;
        }

        for step in &draft.steps {
            for need in &step.needs {
                self.connection().execute(
                    "INSERT INTO edges (id, flow_id, from_step_id, to_step_id, edge_type)
                     VALUES (?1, ?2, ?3, ?4, 'needs')",
                    params![
                        edge_id(&draft.id, need, &step.id),
                        &draft.id,
                        db_step_id(&draft.id, need),
                        db_step_id(&draft.id, &step.id)
                    ],
                )?;
            }
        }

        self.append_event(EventRecord {
            flow_id: Some(draft.id.clone()),
            step_id: None,
            run_id: None,
            event_type: "flow_approved".to_string(),
            payload_json: flow_approved_payload_json(
                &draft.id,
                draft.steps.len(),
                report.edge_count,
            ),
        })?;
        self.touch_project()?;

        Ok(FlowApproval {
            flow_id: draft.id,
            name: draft.name,
            step_count: report.step_count,
            edge_count: report.edge_count,
        })
    }

    pub fn inspect_flow(&self, flow_id: &str) -> Result<FlowInspection, StorageError> {
        let (id, name, status, source_path, schema_version, created_at, updated_at) = self
            .connection()
            .query_row(
                "SELECT id, name, status, source_path, schema_version, created_at, updated_at
                 FROM flows
                 WHERE id = ?1",
                params![flow_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound(format!("flow {flow_id}")))?;

        let mut step_stmt = self.connection().prepare(
            "SELECT id, tool_ref, type, status, reason, params_json, inputs_json, outputs_json
             FROM steps
             WHERE flow_id = ?1
             ORDER BY created_at ASC, id ASC",
        )?;
        let step_rows = step_stmt.query_map(params![flow_id], |row| {
            let id = row.get::<_, String>(0)?;
            Ok(StoredFlowStep {
                local_id: local_step_id(&id),
                id,
                tool_ref: row.get(1)?,
                step_type: row.get(2)?,
                status: row.get(3)?,
                reason: row.get(4)?,
                params_json: row.get(5)?,
                inputs_json: row.get(6)?,
                outputs_json: row.get(7)?,
            })
        })?;
        let mut steps = Vec::new();
        for row in step_rows {
            steps.push(row?);
        }

        let mut edge_stmt = self.connection().prepare(
            "SELECT from_step_id, to_step_id, edge_type
             FROM edges
             WHERE flow_id = ?1
             ORDER BY id ASC",
        )?;
        let edge_rows = edge_stmt.query_map(params![flow_id], |row| {
            let from_step_id = row.get::<_, String>(0)?;
            let to_step_id = row.get::<_, String>(1)?;
            Ok(StoredFlowEdge {
                from_local_id: local_step_id(&from_step_id),
                to_local_id: local_step_id(&to_step_id),
                from_step_id,
                to_step_id,
                edge_type: row.get(2)?,
            })
        })?;
        let mut edges = Vec::new();
        for row in edge_rows {
            edges.push(row?);
        }

        Ok(FlowInspection {
            id,
            name,
            status,
            source_path: source_path.map(PathBuf::from),
            schema_version,
            created_at,
            updated_at,
            steps,
            edges,
        })
    }
}

fn validate_input_reference(
    store: &ProjectStore,
    step_ids: &BTreeSet<String>,
    step_id: &str,
    input_name: &str,
    input_value: &str,
    output_names_by_step: &BTreeMap<String, BTreeSet<String>>,
    issues: &mut Vec<FlowValidationIssue>,
) {
    if input_value.trim().is_empty() {
        issues.push(issue(format!("step {step_id} input {input_name} is empty")));
        return;
    }

    if let Some((producer_step, output_name)) = input_value.split_once('.') {
        if producer_step.is_empty() || output_name.is_empty() {
            issues.push(issue(format!(
                "step {step_id} input {input_name} has invalid step output reference {input_value}"
            )));
        } else if !step_ids.contains(producer_step) {
            issues.push(issue(format!(
                "step {step_id} input {input_name} references unknown producer step {producer_step}"
            )));
        } else if !output_names_by_step
            .get(producer_step)
            .is_some_and(|outputs| outputs.contains(output_name))
        {
            issues.push(issue(format!(
                "step {step_id} input {input_name} references undeclared output {producer_step}.{output_name}"
            )));
        }
        return;
    }

    if let Some(artifact_id) = artifact_ref(input_value) {
        if let Err(error) = store.inspect_artifact(artifact_id) {
            issues.push(issue(format!(
                "step {step_id} input {input_name} references unavailable artifact {artifact_id}: {error}"
            )));
        }
        return;
    }

    issues.push(issue(format!(
        "step {step_id} input {input_name} must reference artifact:<id>, artifact_<id>, or step.output"
    )));
}

fn validate_step_against_tool(
    step: &FlowStepDraft,
    tool: &ExecutableToolSpec,
    issues: &mut Vec<FlowValidationIssue>,
) {
    for (input_name, port) in &tool.inputs {
        if port.required && !step.inputs.contains_key(input_name) {
            issues.push(issue(format!(
                "step {} is missing required input {} for tool {}",
                step.id, input_name, tool.tool_ref
            )));
        }
    }
    for input_name in step.inputs.keys() {
        if !tool.inputs.contains_key(input_name) {
            issues.push(issue(format!(
                "step {} provides unknown input {} for tool {}",
                step.id, input_name, tool.tool_ref
            )));
        }
    }

    for (param_name, param) in &tool.params {
        if param.required
            && step
                .params
                .get(param_name)
                .is_none_or(|value| is_replace_param_placeholder(param_name, value))
        {
            issues.push(issue(format!(
                "step {} is missing required param {} for tool {}",
                step.id, param_name, tool.tool_ref
            )));
        }
    }
    for (param_name, param_value) in &step.params {
        let Some(param) = tool.params.get(param_name) else {
            issues.push(issue(format!(
                "step {} provides unknown param {} for tool {}",
                step.id, param_name, tool.tool_ref
            )));
            continue;
        };
        if is_replace_param_placeholder(param_name, param_value) {
            continue;
        }
        if let Err(error) = validate_param_value(param, param_value) {
            issues.push(issue(format!(
                "step {} param {} invalid for tool {}: {}",
                step.id, param_name, tool.tool_ref, error
            )));
        }
    }

    for output_name in tool.outputs.keys() {
        if !step.outputs.contains_key(output_name) {
            issues.push(issue(format!(
                "step {} is missing required output {} for tool {}",
                step.id, output_name, tool.tool_ref
            )));
        }
    }
    for output_name in step.outputs.keys() {
        if !tool.outputs.contains_key(output_name) {
            issues.push(issue(format!(
                "step {} declares unknown output {} for tool {}",
                step.id, output_name, tool.tool_ref
            )));
        }
    }
}

fn is_replace_param_placeholder(param_name: &str, value: &str) -> bool {
    value == format!("REPLACE_{param_name}")
}

fn artifact_ref(input: &str) -> Option<&str> {
    let trimmed = input.trim();
    if let Some(id) = trimmed.strip_prefix("artifact:") {
        Some(id)
    } else if trimmed.starts_with("artifact_") {
        Some(trimmed)
    } else {
        None
    }
}

fn has_cycle(steps: &[FlowStepDraft]) -> bool {
    let graph = steps
        .iter()
        .map(|step| {
            (
                step.id.as_str(),
                step.needs.iter().map(String::as_str).collect(),
            )
        })
        .collect::<BTreeMap<_, Vec<_>>>();
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();

    for step in steps {
        if dfs_has_cycle(step.id.as_str(), &graph, &mut visiting, &mut visited) {
            return true;
        }
    }
    false
}

fn dfs_has_cycle<'a>(
    step_id: &'a str,
    graph: &BTreeMap<&'a str, Vec<&'a str>>,
    visiting: &mut BTreeSet<&'a str>,
    visited: &mut BTreeSet<&'a str>,
) -> bool {
    if visited.contains(step_id) {
        return false;
    }
    if !visiting.insert(step_id) {
        return true;
    }
    if let Some(needs) = graph.get(step_id) {
        for need in needs {
            if graph.contains_key(need) && dfs_has_cycle(need, graph, visiting, visited) {
                return true;
            }
        }
    }
    visiting.remove(step_id);
    visited.insert(step_id);
    false
}

fn issue(message: impl Into<String>) -> FlowValidationIssue {
    FlowValidationIssue {
        severity: "error".to_string(),
        message: message.into(),
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct FlowApprovedPayload {
    flow_id: String,
    step_count: usize,
    edge_count: usize,
}

fn flow_approved_payload_json(flow_id: &str, step_count: usize, edge_count: usize) -> String {
    serde_json::to_string(&FlowApprovedPayload {
        flow_id: flow_id.to_string(),
        step_count,
        edge_count,
    })
    .expect("flow approved payload serializes to JSON")
}

fn validate_ref_part(label: &str, value: &str) -> Result<(), StorageError> {
    if value.trim().is_empty() {
        return Err(StorageError::InvalidInput(format!(
            "{label} must not be empty"
        )));
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err(StorageError::InvalidInput(format!(
            "{label} may only contain ASCII letters, numbers, underscore, dash, and dot"
        )));
    }
    Ok(())
}

fn map_json(map: &BTreeMap<String, String>) -> String {
    serde_json::to_string(map).expect("flow map serializes to JSON")
}

fn flow_step_json(step: &StoredFlowStep) -> FlowInspectionStepJson {
    FlowInspectionStepJson {
        id: step.id.clone(),
        local_id: step.local_id.clone(),
        tool_ref: step.tool_ref.clone(),
        step_type: step.step_type.clone(),
        status: step.status.clone(),
        reason: step.reason.clone(),
        inputs: serde_json::from_str(&step.inputs_json).expect("stored flow inputs JSON is valid"),
        params: serde_json::from_str(&step.params_json).expect("stored flow params JSON is valid"),
        outputs: serde_json::from_str(&step.outputs_json)
            .expect("stored flow outputs JSON is valid"),
    }
}

fn db_step_id(flow_id: &str, step_id: &str) -> String {
    format!("step:{flow_id}/{step_id}")
}

fn local_step_id(db_step_id: &str) -> String {
    db_step_id
        .rsplit_once('/')
        .map_or_else(|| db_step_id.to_string(), |(_, local)| local.to_string())
}

fn edge_id(flow_id: &str, from_step_id: &str, to_step_id: &str) -> String {
    format!("edge:{flow_id}/{from_step_id}->{to_step_id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{ArtifactImportMode, ArtifactImportRequest, ToolSpec};
    use std::fs;

    fn temp_project_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentflow-flow-registry-{test_name}-{}-{}",
            std::process::id(),
            now_unix_seconds()
        ))
    }

    fn sample_tool() -> &'static str {
        r#"
schema_version: agentflow.tool.v0
namespace: marker
name: marker_survival_scan
version: 0.1.0
maturity: wrapped
description: Scan a candidate marker against survival table
inputs:
  expression_table:
    type: TSV
    required: true
  survival_table:
    type: TSV
    required: true
params:
  gene:
    type: string
    required: true
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/echo
"#
    }

    fn constrained_tool() -> &'static str {
        r#"
schema_version: agentflow.tool.v0
namespace: marker
name: constrained_survival_scan
version: 0.1.0
maturity: wrapped
description: Scan a candidate marker with constrained params
inputs:
  expression_table:
    type: TSV
    required: true
  survival_table:
    type: TSV
    required: true
params:
  mode:
    type: string
    required: true
    enum: [fast, careful]
  gene:
    type: string
    required: true
    pattern: "^[A-Z0-9-]+$"
  retries:
    type: int
    required: true
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/echo
"#
    }

    fn setup_store(test_name: &str) -> (ProjectStore, PathBuf, String, String) {
        let path = temp_project_path(test_name);
        let store = ProjectStore::init(&path, Some("Flows")).unwrap();
        store
            .register_tool(ToolSpec::from_simple_yaml(sample_tool()).unwrap())
            .unwrap();
        let input_path = path.join("expression.tsv");
        fs::write(&input_path, "sample\tTP53\nA\t1.0\n").unwrap();
        let expression_artifact = store
            .import_artifact(ArtifactImportRequest {
                source_path: input_path,
                artifact_type: "TSV".to_string(),
                mode: ArtifactImportMode::Reference,
            })
            .unwrap();
        let survival_path = path.join("survival.tsv");
        fs::write(&survival_path, "sample\ttime\tstatus\nA\t10\t1\n").unwrap();
        let survival_artifact = store
            .import_artifact(ArtifactImportRequest {
                source_path: survival_path,
                artifact_type: "TSV".to_string(),
                mode: ArtifactImportMode::Reference,
            })
            .unwrap();
        (
            store,
            path,
            expression_artifact.summary.id,
            survival_artifact.summary.id,
        )
    }

    fn constrained_flow(
        expression_artifact_id: &str,
        survival_artifact_id: &str,
        mode: &str,
        gene: &str,
        retries: &str,
    ) -> String {
        format!(
            r#"
schema_version: agentflow.flow.v0
id: constrained_demo
name: Constrained demo
steps:
  - id: scan
    tool: marker/constrained_survival_scan
    reason: Evaluate constrained marker signal
    needs: []
    inputs:
      expression_table: {expression_artifact_id}
      survival_table: {survival_artifact_id}
    params:
      mode: {mode}
      gene: {gene}
      retries: {retries}
    outputs:
      report: marker_report
"#
        )
    }

    fn sample_flow(expression_artifact_id: &str, survival_artifact_id: Option<&str>) -> String {
        let survival_input = survival_artifact_id.map_or_else(String::new, |artifact_id| {
            format!("      survival_table: {artifact_id}\n")
        });
        format!(
            r#"
schema_version: agentflow.flow.v0
id: marker_demo
name: Marker demo
steps:
  - id: scan
    tool: marker/marker_survival_scan
    reason: Evaluate TP53 marker signal
    needs: []
    inputs:
      expression_table: {expression_artifact_id}
{survival_input}    params:
      gene: TP53
    outputs:
      report: marker_report
"#
        )
    }

    #[test]
    fn parses_simple_flow_yaml() {
        let draft =
            FlowDraft::from_simple_yaml(&sample_flow("artifact_1", Some("artifact_2"))).unwrap();

        assert_eq!(draft.schema_version, agentflow_schemas::FLOW_SCHEMA_V0);
        assert_eq!(draft.id, "marker_demo");
        assert_eq!(draft.steps.len(), 1);
        assert_eq!(draft.steps[0].tool_ref, "marker/marker_survival_scan");
        assert_eq!(
            draft.steps[0].inputs.get("expression_table").unwrap(),
            "artifact_1"
        );
    }

    #[test]
    fn parses_inline_flow_yaml_equivalent_to_block_form() {
        let block = FlowDraft::from_simple_yaml(
            r#"
schema_version: agentflow.flow.v0
id: inline_demo
name: Inline demo
steps:
  - id: prep
    tool: marker/prep
    needs: []
    inputs:
      expr: artifact_1
    params:
      gene: TP53
    outputs:
      report: prep_report
  - id: scan
    tool: marker/scan
    needs:
      - prep
      - qc
    inputs:
      expr: prep.report
    params:
      gene: TP53
    outputs:
      report: marker_report
"#,
        )
        .unwrap();
        let mut inline = FlowDraft::from_simple_yaml(
            r#"
schema_version: agentflow.flow.v0
id: inline_demo
name: Inline demo
steps:
  - {id: prep, tool: marker/prep, needs: [], inputs: {expr: artifact_1}, params: {gene: TP53}, outputs: {report: prep_report}}
  - {id: scan, tool: marker/scan, needs: [prep, qc], inputs: {expr: prep.report}, params: {gene: TP53}, outputs: {report: marker_report}}
"#,
        )
        .unwrap();

        inline.source_text = block.source_text.clone();
        assert_eq!(inline, block);
    }

    #[test]
    fn flow_yaml_validation_errors_are_invalid_input_with_location() {
        let err = FlowDraft::from_simple_yaml(
            r#"
schema_version: agentflow.flow.v0
id: bad id
name: Bad flow
steps: []
"#,
        )
        .unwrap_err();

        assert!(matches!(err, StorageError::InvalidInput(_)));
        let message = err.to_string();
        assert!(message.contains("flow id"), "{message}");
        assert!(message.contains("line"), "{message}");
        assert!(message.contains("column"), "{message}");
    }

    #[test]
    fn validate_approve_and_inspect_flow() {
        let (store, path, expression_id, survival_id) = setup_store("approve");
        let draft =
            FlowDraft::from_simple_yaml(&sample_flow(&expression_id, Some(&survival_id))).unwrap();

        let report = store.validate_flow(&draft);
        assert!(report.valid, "unexpected issues: {:?}", report.issues);

        let approval = store
            .approve_flow(draft, Some(path.join("flow.yaml").as_path()))
            .unwrap();
        assert_eq!(approval.flow_id, "marker_demo");
        assert_eq!(approval.step_count, 1);

        let inspection = store.inspect_flow("marker_demo").unwrap();
        assert_eq!(inspection.status, "approved");
        assert_eq!(inspection.steps.len(), 1);
        assert_eq!(inspection.steps[0].local_id, "scan");
        assert!(inspection
            .to_json()
            .contains("agentflow.flow_inspection.v0"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn validation_accepts_param_values_that_satisfy_constraints() {
        let (store, path, expression_id, survival_id) = setup_store("valid-param-constraints");
        store
            .register_tool(ToolSpec::from_simple_yaml(constrained_tool()).unwrap())
            .unwrap();
        let draft = FlowDraft::from_simple_yaml(&constrained_flow(
            &expression_id,
            &survival_id,
            "fast",
            "TP53-1",
            "3",
        ))
        .unwrap();

        let report = store.validate_flow(&draft);
        assert!(report.valid, "unexpected issues: {:?}", report.issues);

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn validation_rejects_invalid_param_type_enum_and_pattern_values() {
        let (store, path, expression_id, survival_id) = setup_store("invalid-param-constraints");
        store
            .register_tool(ToolSpec::from_simple_yaml(constrained_tool()).unwrap())
            .unwrap();
        let draft = FlowDraft::from_simple_yaml(&constrained_flow(
            &expression_id,
            &survival_id,
            "slow",
            "TP53!",
            "many",
        ))
        .unwrap();

        let report = store.validate_flow(&draft);
        assert!(!report.valid);
        let message = report.error_message();
        assert!(message.contains("param mode"), "{message}");
        assert!(message.contains("must be one of"), "{message}");
        assert!(message.contains("param gene"), "{message}");
        assert!(message.contains("must match pattern"), "{message}");
        assert!(message.contains("param retries"), "{message}");
        assert!(message.contains("must be an int"), "{message}");

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn validation_treats_replace_param_placeholder_as_unfilled_without_pattern_error() {
        let (store, path, expression_id, survival_id) = setup_store("replace-param-unfilled");
        store
            .register_tool(ToolSpec::from_simple_yaml(constrained_tool()).unwrap())
            .unwrap();
        let draft = FlowDraft::from_simple_yaml(&constrained_flow(
            &expression_id,
            &survival_id,
            "fast",
            "REPLACE_gene",
            "3",
        ))
        .unwrap();

        let report = store.validate_flow(&draft);
        assert!(!report.valid);
        let message = report.error_message();
        assert!(message.contains("missing required param gene"), "{message}");
        assert!(!message.contains("must match pattern"), "{message}");

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn validation_rejects_missing_tool_and_artifact() {
        let (store, path, _expression_id, survival_id) = setup_store("missing");
        let draft =
            FlowDraft::from_simple_yaml(&sample_flow("artifact_missing", Some(&survival_id)))
                .unwrap();

        let report = store.validate_flow(&draft);
        assert!(!report.valid);
        assert!(report.error_message().contains("unavailable artifact"));

        let mut missing_tool = draft.clone();
        missing_tool.steps[0].tool_ref = "marker/missing_tool".to_string();
        let report = store.validate_flow(&missing_tool);
        assert!(!report.valid);
        assert!(report
            .error_message()
            .contains("unavailable or non-executable tool"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn validation_rejects_missing_required_tool_input() {
        let (store, path, expression_id, _survival_id) = setup_store("missing-required-input");
        let draft = FlowDraft::from_simple_yaml(&sample_flow(&expression_id, None)).unwrap();

        let report = store.validate_flow(&draft);
        assert!(!report.valid);
        assert!(report
            .error_message()
            .contains("missing required input survival_table"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn validation_rejects_dependency_cycle() {
        let (store, path, expression_id, survival_id) = setup_store("cycle");
        let source = format!(
            r#"
schema_version: agentflow.flow.v0
id: cycle_demo
name: Cycle demo
steps:
  - id: a
    tool: marker/marker_survival_scan
    needs: [b]
    inputs:
      expression_table: {expression_id}
      survival_table: {survival_id}
    params:
      gene: TP53
    outputs:
      report: report_a
  - id: b
    tool: marker/marker_survival_scan
    needs: [a]
    inputs:
      expression_table: a.report
      survival_table: {survival_id}
    params:
      gene: TP53
    outputs:
      report: report_b
"#
        );
        let draft = FlowDraft::from_simple_yaml(&source).unwrap();

        let report = store.validate_flow(&draft);
        assert!(!report.valid);
        assert!(report.error_message().contains("dependency cycle"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn flow_validation_json_is_exact_byte_and_serde_readable() {
        let report = FlowValidationReport {
            flow_id: "flow_1".to_string(),
            name: "Flow \"A\"".to_string(),
            valid: false,
            step_count: 1,
            edge_count: 0,
            issues: vec![FlowValidationIssue {
                severity: "error".to_string(),
                message: "missing tool".to_string(),
            }],
        };

        let json = report.to_json();
        assert_eq!(
            json,
            "{\"schema_version\":\"agentflow.flow_validation.v0\",\"flow_id\":\"flow_1\",\"name\":\"Flow \\\"A\\\"\",\"valid\":false,\"step_count\":1,\"edge_count\":0,\"issues\":[{\"severity\":\"error\",\"message\":\"missing tool\"}]}"
        );

        let payload: FlowValidationReportJson = serde_json::from_str(&json).unwrap();
        assert_eq!(payload.issues[0].message, "missing tool");
    }

    #[test]
    fn flow_inspection_json_is_exact_byte_and_embeds_step_maps() {
        let inspection = FlowInspection {
            id: "flow_1".to_string(),
            name: "Flow One".to_string(),
            status: "approved".to_string(),
            source_path: Some(PathBuf::from("/tmp/flow.yaml")),
            schema_version: agentflow_schemas::FLOW_SCHEMA_V0.to_string(),
            created_at: 1,
            updated_at: 2,
            steps: vec![StoredFlowStep {
                id: "step:flow_1/scan".to_string(),
                local_id: "scan".to_string(),
                tool_ref: Some("marker/scan".to_string()),
                step_type: "analysis".to_string(),
                status: "draft".to_string(),
                reason: Some("Evaluate marker".to_string()),
                params_json: "{\"gene\":\"TP53\"}".to_string(),
                inputs_json: "{\"table\":\"artifact_1\"}".to_string(),
                outputs_json: "{\"report\":\"marker_report\"}".to_string(),
            }],
            edges: vec![StoredFlowEdge {
                from_step_id: "step:flow_1/prep".to_string(),
                to_step_id: "step:flow_1/scan".to_string(),
                from_local_id: "prep".to_string(),
                to_local_id: "scan".to_string(),
                edge_type: "needs".to_string(),
            }],
        };

        assert_eq!(
            inspection.to_json(),
            "{\"schema_version\":\"agentflow.flow_inspection.v0\",\"flow\":{\"id\":\"flow_1\",\"name\":\"Flow One\",\"status\":\"approved\",\"source_path\":\"/tmp/flow.yaml\",\"flow_schema_version\":\"agentflow.flow.v0\",\"created_at\":1,\"updated_at\":2,\"steps\":[{\"id\":\"step:flow_1/scan\",\"local_id\":\"scan\",\"tool_ref\":\"marker/scan\",\"type\":\"analysis\",\"status\":\"draft\",\"reason\":\"Evaluate marker\",\"inputs\":{\"table\":\"artifact_1\"},\"params\":{\"gene\":\"TP53\"},\"outputs\":{\"report\":\"marker_report\"}}],\"edges\":[{\"from\":\"prep\",\"to\":\"scan\",\"type\":\"needs\"}]}}"
        );

        let payload: FlowApprovedPayload =
            serde_json::from_str("{\"flow_id\":\"flow_1\",\"step_count\":1,\"edge_count\":0}")
                .unwrap();
        assert_eq!(payload.flow_id, "flow_1");
    }

    const QC_QUANTIFY_MODULE: &str = r#"
schema_version: agentflow.module.v0
namespace: bio
name: qc_then_quantify
version: 0.1.0
description: QC then quantify.
inputs:
  counts:
    type: RawCounts
outputs:
  expression:
    type: ExpressionTable
    from: quant_out
steps:
  - id: qc
    tool: bio/qc
    inputs:
      counts: counts
    outputs:
      clean: qc_clean
  - id: quant
    tool: bio/quantify
    needs: [qc]
    inputs:
      counts: qc_clean
    outputs:
      expression: quant_out
"#;

    fn qc_module_map() -> BTreeMap<String, ModuleSpec> {
        let spec = ModuleSpec::from_simple_yaml(QC_QUANTIFY_MODULE).unwrap();
        BTreeMap::from([(spec.module_ref(), spec)])
    }

    fn step<'a>(draft: &'a FlowDraft, id: &str) -> &'a FlowStepDraft {
        draft.steps.iter().find(|s| s.id == id).unwrap_or_else(|| {
            panic!(
                "expanded flow should contain step {id}; got {:?}",
                draft
                    .steps
                    .iter()
                    .map(|s| s.id.as_str())
                    .collect::<Vec<_>>()
            )
        })
    }

    #[test]
    fn expands_a_module_step_and_wires_a_downstream_consumer() {
        let flow = r#"
schema_version: agentflow.flow.v0
id: f1
name: Flow with module
steps:
  - id: prep
    module: bio/qc_then_quantify
    inputs:
      counts: artifact_raw
  - id: analyze
    tool: bio/analyze
    needs: [prep]
    inputs:
      expression: prep.expression
    outputs:
      report: analyze_report
"#;
        let draft = FlowDraft::from_simple_yaml_with_modules(flow, &qc_module_map()).unwrap();
        // prep expands to two prefixed tool steps; analyze stays.
        assert_eq!(draft.steps.len(), 3);

        let qc = step(&draft, "prep__qc");
        assert_eq!(qc.tool_ref, "bio/qc");
        assert_eq!(qc.inputs["counts"], "artifact_raw"); // external port -> binding
        assert_eq!(qc.outputs["clean"], "prep__qc_clean");
        assert!(qc.needs.is_empty());

        let quant = step(&draft, "prep__quant");
        assert_eq!(quant.needs, vec!["prep__qc".to_string()]);
        assert_eq!(quant.inputs["counts"], "prep__qc_clean");
        assert_eq!(quant.outputs["expression"], "prep__quant_out");

        let analyze = step(&draft, "analyze");
        // `prep.expression` -> the exposed artifact; needs -> the producer.
        assert_eq!(analyze.inputs["expression"], "prep__quant_out");
        assert_eq!(analyze.needs, vec!["prep__quant".to_string()]);
    }

    #[test]
    fn module_step_upstream_needs_propagate_to_source_steps() {
        let flow = r#"
schema_version: agentflow.flow.v0
id: f2
name: Flow with upstream
steps:
  - id: importer
    tool: bio/import
    outputs:
      counts: importer_out
  - id: prep
    module: bio/qc_then_quantify
    needs: [importer]
    inputs:
      counts: importer_out
"#;
        let draft = FlowDraft::from_simple_yaml_with_modules(flow, &qc_module_map()).unwrap();
        // The source step (qc) inherits the module step's upstream need; the
        // deeper step (quant) depends through qc.
        assert_eq!(step(&draft, "prep__qc").needs, vec!["importer".to_string()]);
        assert_eq!(
            step(&draft, "prep__quant").needs,
            vec!["prep__qc".to_string()]
        );
        assert_eq!(step(&draft, "prep__qc").inputs["counts"], "importer_out");
    }

    #[test]
    fn two_module_instances_do_not_collide() {
        let flow = r#"
schema_version: agentflow.flow.v0
id: f3
name: Two instances
steps:
  - id: a
    module: bio/qc_then_quantify
    inputs:
      counts: artifact_a
  - id: b
    module: bio/qc_then_quantify
    inputs:
      counts: artifact_b
"#;
        let draft = FlowDraft::from_simple_yaml_with_modules(flow, &qc_module_map()).unwrap();
        assert_eq!(draft.steps.len(), 4);
        assert_eq!(step(&draft, "a__qc").inputs["counts"], "artifact_a");
        assert_eq!(step(&draft, "b__qc").inputs["counts"], "artifact_b");
        assert_eq!(
            step(&draft, "a__quant").outputs["expression"],
            "a__quant_out"
        );
        assert_eq!(
            step(&draft, "b__quant").outputs["expression"],
            "b__quant_out"
        );
    }

    #[test]
    fn rejects_unknown_module_reference() {
        let flow = r#"
schema_version: agentflow.flow.v0
id: f4
name: Unknown module
steps:
  - id: prep
    module: bio/missing
    inputs:
      counts: artifact_raw
"#;
        let err = FlowDraft::from_simple_yaml_with_modules(flow, &qc_module_map()).unwrap_err();
        assert!(err.to_string().contains("which was not provided"));
    }

    #[test]
    fn rejects_step_declaring_both_tool_and_module() {
        let flow = r#"
schema_version: agentflow.flow.v0
id: f5
name: Both
steps:
  - id: prep
    tool: bio/qc
    module: bio/qc_then_quantify
    inputs:
      counts: artifact_raw
"#;
        let err = FlowDraft::from_simple_yaml_with_modules(flow, &qc_module_map()).unwrap_err();
        assert!(err.to_string().contains("declares both tool and module"));
    }

    #[test]
    fn rejects_dotted_module_step_id() {
        // A '.' in a module instance id would break `instance.port` parsing.
        let flow = r#"
schema_version: agentflow.flow.v0
id: f7
name: Dotted id
steps:
  - id: prep.v2
    module: bio/qc_then_quantify
    inputs:
      counts: artifact_raw
"#;
        let err = FlowDraft::from_simple_yaml_with_modules(flow, &qc_module_map()).unwrap_err();
        assert!(err.to_string().contains("must not contain '.'"));
    }

    #[test]
    fn plain_parse_rejects_a_module_step() {
        let flow = r#"
schema_version: agentflow.flow.v0
id: f6
name: No modules provided
steps:
  - id: prep
    module: bio/qc_then_quantify
    inputs:
      counts: artifact_raw
"#;
        let err = FlowDraft::from_simple_yaml(flow).unwrap_err();
        assert!(err.to_string().contains("which was not provided"));
    }
}
