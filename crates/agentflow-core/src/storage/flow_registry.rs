use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rusqlite::{params, OptionalExtension};
use serde::Deserialize;

use crate::domain::StepStatus;

use super::project_store::{now_unix_seconds, EventRecord, ProjectStore, StorageError};
use super::tool_registry::ExecutableToolSpec;
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
    pub fn from_simple_yaml(source_text: &str) -> Result<Self, StorageError> {
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
            steps: raw.into_steps(source_text)?,
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

impl RawFlowDraft {
    fn into_steps(self, source_text: &str) -> Result<Vec<FlowStepDraft>, StorageError> {
        self.steps
            .into_iter()
            .map(|step| step.into_step(source_text))
            .collect()
    }
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
        let issues = self
            .issues
            .iter()
            .map(|issue| {
                format!(
                    "{{\"severity\":\"{}\",\"message\":\"{}\"}}",
                    escape_json(&issue.severity),
                    escape_json(&issue.message)
                )
            })
            .collect::<Vec<_>>()
            .join(",");

        format!(
            concat!(
                "{{",
                "\"schema_version\":\"{}\",",
                "\"flow_id\":\"{}\",",
                "\"name\":\"{}\",",
                "\"valid\":{},",
                "\"step_count\":{},",
                "\"edge_count\":{},",
                "\"issues\":[{}]",
                "}}"
            ),
            agentflow_schemas::FLOW_VALIDATION_JSON_SCHEMA_V0,
            escape_json(&self.flow_id),
            escape_json(&self.name),
            self.valid,
            self.step_count,
            self.edge_count,
            issues
        )
    }

    fn error_message(&self) -> String {
        self.issues
            .iter()
            .map(|issue| issue.message.clone())
            .collect::<Vec<_>>()
            .join("; ")
    }
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
        let steps = self
            .steps
            .iter()
            .map(|step| {
                format!(
                    concat!(
                        "{{",
                        "\"id\":\"{}\",",
                        "\"local_id\":\"{}\",",
                        "\"tool_ref\":{},",
                        "\"type\":\"{}\",",
                        "\"status\":\"{}\",",
                        "\"reason\":{},",
                        "\"inputs\":{},",
                        "\"params\":{},",
                        "\"outputs\":{}",
                        "}}"
                    ),
                    escape_json(&step.id),
                    escape_json(&step.local_id),
                    optional_json_string(step.tool_ref.as_deref()),
                    escape_json(&step.step_type),
                    escape_json(&step.status),
                    optional_json_string(step.reason.as_deref()),
                    step.inputs_json,
                    step.params_json,
                    step.outputs_json
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let edges = self
            .edges
            .iter()
            .map(|edge| {
                format!(
                    "{{\"from\":\"{}\",\"to\":\"{}\",\"type\":\"{}\"}}",
                    escape_json(&edge.from_local_id),
                    escape_json(&edge.to_local_id),
                    escape_json(&edge.edge_type)
                )
            })
            .collect::<Vec<_>>()
            .join(",");

        format!(
            concat!(
                "{{",
                "\"schema_version\":\"{}\",",
                "\"flow\":{{",
                "\"id\":\"{}\",",
                "\"name\":\"{}\",",
                "\"status\":\"{}\",",
                "\"source_path\":{},",
                "\"flow_schema_version\":\"{}\",",
                "\"created_at\":{},",
                "\"updated_at\":{},",
                "\"steps\":[{}],",
                "\"edges\":[{}]",
                "}}",
                "}}"
            ),
            agentflow_schemas::FLOW_INSPECTION_JSON_SCHEMA_V0,
            escape_json(&self.id),
            escape_json(&self.name),
            escape_json(&self.status),
            optional_json_string(
                self.source_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .as_deref()
            ),
            escape_json(&self.schema_version),
            self.created_at,
            self.updated_at,
            steps,
            edges
        )
    }
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
            payload_json: format!(
                "{{\"flow_id\":\"{}\",\"step_count\":{},\"edge_count\":{}}}",
                escape_json(&draft.id),
                draft.steps.len(),
                report.edge_count
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
        if param.required && !step.params.contains_key(param_name) {
            issues.push(issue(format!(
                "step {} is missing required param {} for tool {}",
                step.id, param_name, tool.tool_ref
            )));
        }
    }
    for param_name in step.params.keys() {
        if !tool.params.contains_key(param_name) {
            issues.push(issue(format!(
                "step {} provides unknown param {} for tool {}",
                step.id, param_name, tool.tool_ref
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
    let fields = map
        .iter()
        .map(|(key, value)| format!("\"{}\":\"{}\"", escape_json(key), escape_json(value)))
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{fields}}}")
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

fn optional_json_string(value: Option<&str>) -> String {
    value.map_or_else(
        || "null".to_string(),
        |inner| format!("\"{}\"", escape_json(inner)),
    )
}

fn escape_json(input: &str) -> String {
    let mut output = String::new();
    for ch in input.chars() {
        match ch {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            _ => output.push(ch),
        }
    }
    output
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
}
