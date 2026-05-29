use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{params, OptionalExtension};

use crate::domain::StepStatus;
use crate::storage::{EventRecord, ProjectStore, StorageError};
use crate::storage::{FlowDraft, FlowStepDraft, StoredFlowStep};

const GRAPH_PATCH_PROPOSED_EVENT: &str = "graph_patch_proposed";
const GRAPH_PATCH_APPROVED_EVENT: &str = "graph_patch_approved";
const GRAPH_PATCH_REJECTED_EVENT: &str = "graph_patch_rejected";
const GRAPH_PATCH_APPLIED_EVENT: &str = "graph_patch_applied";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphPatchRecord {
    pub id: String,
    pub flow_id: String,
    pub title: String,
    pub reason: String,
    pub patch_json: String,
    pub status: String,
    pub decision_reason: Option<String>,
    pub created_at: i64,
    pub decided_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphPatchApplication {
    pub patch_id: String,
    pub flow_id: String,
    pub applied_steps: Vec<String>,
    pub applied_edges: Vec<(String, String)>,
    pub updated_steps: Vec<String>,
    pub invalidated_steps: Vec<String>,
}

impl ProjectStore {
    pub fn propose_graph_patch(
        &self,
        flow_id: &str,
        title: &str,
        reason: &str,
        patch_json: &str,
    ) -> Result<GraphPatchRecord, StorageError> {
        let flow_id = validate_flow_id(flow_id)?;
        let title = validate_non_empty("graph patch title", title)?;
        let reason = validate_non_empty("graph patch reason", reason)?;
        let patch_json = validate_non_empty("graph patch patch_json", patch_json)?;
        ensure_flow_exists(self, flow_id)?;

        let id = self.append_event(EventRecord {
            flow_id: Some(flow_id.to_string()),
            step_id: None,
            run_id: None,
            event_type: GRAPH_PATCH_PROPOSED_EVENT.to_string(),
            payload_json: proposal_payload_json(title, reason, patch_json),
        })?;
        self.touch_project()?;
        self.inspect_graph_patch(&id)
    }

    pub fn list_graph_patches(&self, flow_id: &str) -> Result<Vec<GraphPatchRecord>, StorageError> {
        let flow_id = validate_flow_id(flow_id)?;
        ensure_flow_exists(self, flow_id)?;

        let mut stmt = self.connection().prepare(
            "SELECT id, event_type, flow_id, payload_json, created_at
             FROM events
             WHERE flow_id = ?1
               AND event_type IN (?2, ?3, ?4, ?5)
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map(
            params![
                flow_id,
                GRAPH_PATCH_PROPOSED_EVENT,
                GRAPH_PATCH_APPROVED_EVENT,
                GRAPH_PATCH_REJECTED_EVENT,
                GRAPH_PATCH_APPLIED_EVENT
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )?;

        let mut patches = Vec::new();
        for row in rows {
            let (event_id, event_type, event_flow_id, payload_json, created_at) = row?;
            match event_type.as_str() {
                GRAPH_PATCH_PROPOSED_EVENT => {
                    let flow_id = event_flow_id.ok_or_else(|| {
                        StorageError::InvalidInput(format!(
                            "graph patch proposal {event_id} is missing flow_id"
                        ))
                    })?;
                    patches.push(graph_patch_from_proposal(
                        event_id,
                        flow_id,
                        &payload_json,
                        created_at,
                    )?);
                }
                GRAPH_PATCH_APPROVED_EVENT
                | GRAPH_PATCH_REJECTED_EVENT
                | GRAPH_PATCH_APPLIED_EVENT => {
                    let patch_id =
                        json_string_field(&payload_json, "patch_id").ok_or_else(|| {
                            StorageError::InvalidInput(format!(
                                "graph patch decision {event_id} is missing patch_id"
                            ))
                        })?;
                    if let Some(patch) = patches.iter_mut().find(|patch| patch.id == patch_id) {
                        apply_decision(patch, &event_type, &payload_json, created_at)?;
                    }
                }
                _ => {}
            }
        }

        Ok(patches)
    }

    pub fn approve_graph_patch(&self, patch_id: &str) -> Result<GraphPatchRecord, StorageError> {
        let patch_id = validate_non_empty("graph patch id", patch_id)?;
        let patch = self.inspect_graph_patch(patch_id)?;
        ensure_patch_pending(&patch)?;

        self.append_event(EventRecord {
            flow_id: Some(patch.flow_id.clone()),
            step_id: None,
            run_id: None,
            event_type: GRAPH_PATCH_APPROVED_EVENT.to_string(),
            payload_json: decision_payload_json(patch_id, None),
        })?;
        self.touch_project()?;
        self.inspect_graph_patch(patch_id)
    }

    pub fn reject_graph_patch(
        &self,
        patch_id: &str,
        reason: &str,
    ) -> Result<GraphPatchRecord, StorageError> {
        let patch_id = validate_non_empty("graph patch id", patch_id)?;
        let reason = validate_non_empty("graph patch rejection reason", reason)?;
        let patch = self.inspect_graph_patch(patch_id)?;
        ensure_patch_pending(&patch)?;

        self.append_event(EventRecord {
            flow_id: Some(patch.flow_id.clone()),
            step_id: None,
            run_id: None,
            event_type: GRAPH_PATCH_REJECTED_EVENT.to_string(),
            payload_json: decision_payload_json(patch_id, Some(reason)),
        })?;
        self.touch_project()?;
        self.inspect_graph_patch(patch_id)
    }

    pub fn apply_graph_patch(&self, patch_id: &str) -> Result<GraphPatchApplication, StorageError> {
        let patch_id = validate_non_empty("graph patch id", patch_id)?;
        let patch = self.inspect_graph_patch(patch_id)?;
        ensure_patch_approved(&patch)?;

        let operations = parse_graph_patch_operations(&patch.patch_json)?;
        self.connection().execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            let application = materialize_graph_patch(self, &patch, operations)?;
            self.append_event(EventRecord {
                flow_id: Some(patch.flow_id.clone()),
                step_id: None,
                run_id: None,
                event_type: GRAPH_PATCH_APPLIED_EVENT.to_string(),
                payload_json: application_payload_json(&application),
            })?;
            self.touch_project()?;
            Ok(application)
        })();

        match result {
            Ok(application) => {
                self.connection().execute_batch("COMMIT")?;
                Ok(application)
            }
            Err(error) => {
                let _ = self.connection().execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    fn inspect_graph_patch(&self, patch_id: &str) -> Result<GraphPatchRecord, StorageError> {
        let patch_id = validate_non_empty("graph patch id", patch_id)?;
        let (flow_id, payload_json, created_at) = self
            .connection()
            .query_row(
                "SELECT flow_id, payload_json, created_at
                 FROM events
                 WHERE id = ?1 AND event_type = ?2",
                params![patch_id, GRAPH_PATCH_PROPOSED_EVENT],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound(format!("graph patch {patch_id}")))?;
        let flow_id = flow_id.ok_or_else(|| {
            StorageError::InvalidInput(format!(
                "graph patch proposal {patch_id} is missing flow_id"
            ))
        })?;

        let mut patch = graph_patch_from_proposal(
            patch_id.to_string(),
            flow_id.clone(),
            &payload_json,
            created_at,
        )?;

        let mut stmt = self.connection().prepare(
            "SELECT event_type, payload_json, created_at
             FROM events
             WHERE flow_id = ?1
               AND event_type IN (?2, ?3, ?4)
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map(
            params![
                flow_id,
                GRAPH_PATCH_APPROVED_EVENT,
                GRAPH_PATCH_REJECTED_EVENT,
                GRAPH_PATCH_APPLIED_EVENT
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )?;

        for row in rows {
            let (event_type, payload_json, created_at) = row?;
            if json_string_field(&payload_json, "patch_id").as_deref() == Some(patch_id) {
                apply_decision(&mut patch, &event_type, &payload_json, created_at)?;
            }
        }

        Ok(patch)
    }
}

fn ensure_flow_exists(store: &ProjectStore, flow_id: &str) -> Result<(), StorageError> {
    let exists: Option<String> = store
        .connection()
        .query_row(
            "SELECT id FROM flows WHERE id = ?1",
            params![flow_id],
            |row| row.get(0),
        )
        .optional()?;

    if exists.is_some() {
        Ok(())
    } else {
        Err(StorageError::NotFound(format!("flow {flow_id}")))
    }
}

fn graph_patch_from_proposal(
    id: String,
    flow_id: String,
    payload_json: &str,
    created_at: i64,
) -> Result<GraphPatchRecord, StorageError> {
    let title = json_string_field(payload_json, "title").ok_or_else(|| {
        StorageError::InvalidInput(format!("graph patch proposal {id} is missing title"))
    })?;
    let reason = json_string_field(payload_json, "reason").ok_or_else(|| {
        StorageError::InvalidInput(format!("graph patch proposal {id} is missing reason"))
    })?;
    let patch_json = json_string_field(payload_json, "patch_json").ok_or_else(|| {
        StorageError::InvalidInput(format!("graph patch proposal {id} is missing patch_json"))
    })?;

    Ok(GraphPatchRecord {
        id,
        flow_id,
        title,
        reason,
        patch_json,
        status: "pending".to_string(),
        decision_reason: None,
        created_at,
        decided_at: None,
    })
}

fn apply_decision(
    patch: &mut GraphPatchRecord,
    event_type: &str,
    payload_json: &str,
    created_at: i64,
) -> Result<(), StorageError> {
    if matches!(patch.status.as_str(), "rejected" | "applied") {
        return Ok(());
    }

    match event_type {
        GRAPH_PATCH_APPROVED_EVENT => {
            if patch.status != "pending" {
                return Ok(());
            }
            patch.status = "approved".to_string();
            patch.decision_reason = None;
            patch.decided_at = Some(created_at);
            Ok(())
        }
        GRAPH_PATCH_REJECTED_EVENT => {
            patch.status = "rejected".to_string();
            patch.decision_reason = json_string_field(payload_json, "reason");
            patch.decided_at = Some(created_at);
            Ok(())
        }
        GRAPH_PATCH_APPLIED_EVENT => {
            if patch.status == "approved" {
                patch.status = "applied".to_string();
                patch.decided_at = Some(created_at);
            }
            Ok(())
        }
        other => Err(StorageError::InvalidInput(format!(
            "unsupported graph patch decision event_type {other}"
        ))),
    }
}

fn ensure_patch_pending(patch: &GraphPatchRecord) -> Result<(), StorageError> {
    if patch.status == "pending" {
        Ok(())
    } else {
        Err(StorageError::InvalidInput(format!(
            "graph patch {} has already been {}",
            patch.id, patch.status
        )))
    }
}

fn ensure_patch_approved(patch: &GraphPatchRecord) -> Result<(), StorageError> {
    if patch.status == "approved" {
        Ok(())
    } else {
        Err(StorageError::InvalidInput(format!(
            "graph patch {} must be approved before apply; current status is {}",
            patch.id, patch.status
        )))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GraphPatchOperation {
    AddStep(FlowStepDraft),
    AddEdge {
        from: String,
        to: String,
    },
    UpdateParams {
        step: String,
        params: BTreeMap<String, String>,
    },
}

fn materialize_graph_patch(
    store: &ProjectStore,
    patch: &GraphPatchRecord,
    operations: Vec<GraphPatchOperation>,
) -> Result<GraphPatchApplication, StorageError> {
    if operations.is_empty() {
        return Err(StorageError::InvalidInput(
            "graph patch must contain at least one operation".to_string(),
        ));
    }

    let flow = store.inspect_flow(&patch.flow_id)?;
    let mut draft = flow_inspection_to_draft(&flow)?;
    let mut known_steps = draft
        .steps
        .iter()
        .map(|step| step.id.clone())
        .collect::<BTreeSet<_>>();
    let original_steps = flow
        .steps
        .iter()
        .map(|step| (step.local_id.clone(), step.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut added_steps = BTreeSet::new();
    let mut updated_steps = BTreeSet::new();
    let mut extra_edges = Vec::<(String, String)>::new();

    for operation in operations {
        match operation {
            GraphPatchOperation::AddStep(step) => {
                validate_ref_part("graph patch step id", &step.id)?;
                if known_steps.contains(&step.id) || !added_steps.insert(step.id.clone()) {
                    return Err(StorageError::InvalidInput(format!(
                        "graph patch add_step duplicates step {}",
                        step.id
                    )));
                }
                known_steps.insert(step.id.clone());
                draft.steps.push(step);
            }
            GraphPatchOperation::AddEdge { from, to } => {
                validate_ref_part("graph patch edge from", &from)?;
                validate_ref_part("graph patch edge to", &to)?;
                extra_edges.push((from, to));
            }
            GraphPatchOperation::UpdateParams { step, params } => {
                validate_ref_part("graph patch update_params step", &step)?;
                if params.is_empty() {
                    return Err(StorageError::InvalidInput(
                        "graph patch update_params must include at least one param".to_string(),
                    ));
                }
                let draft_step = draft
                    .steps
                    .iter_mut()
                    .find(|draft_step| draft_step.id == step)
                    .ok_or_else(|| {
                        StorageError::InvalidInput(format!(
                            "graph patch update_params references unknown step {step}"
                        ))
                    })?;
                for (key, value) in params {
                    if draft_step.params.get(&key) != Some(&value) {
                        draft_step.params.insert(key, value);
                        updated_steps.insert(step.clone());
                    }
                }
            }
        }
    }

    for (from, to) in extra_edges {
        if !known_steps.contains(&from) {
            return Err(StorageError::InvalidInput(format!(
                "graph patch add_edge references unknown from step {from}"
            )));
        }
        if !known_steps.contains(&to) {
            return Err(StorageError::InvalidInput(format!(
                "graph patch add_edge references unknown to step {to}"
            )));
        }
        if !added_steps.contains(&to) {
            return Err(StorageError::InvalidInput(format!(
                "graph patch add_edge can only target newly added steps in this version; got {to}"
            )));
        }
        let step = draft
            .steps
            .iter_mut()
            .find(|step| step.id == to)
            .expect("known step checked above");
        if !step.needs.contains(&from) {
            step.needs.push(from);
        }
    }

    for step in &draft.steps {
        for need in &step.needs {
            if !known_steps.contains(need) {
                return Err(StorageError::InvalidInput(format!(
                    "graph patch step {} needs unknown step {}",
                    step.id, need
                )));
            }
        }
    }

    let validation = store.validate_flow(&draft);
    if !validation.valid {
        return Err(StorageError::InvalidInput(format!(
            "graph patch validation failed: {}",
            validation
                .issues
                .iter()
                .map(|issue| issue.message.clone())
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }

    let applied_steps = draft
        .steps
        .iter()
        .filter(|step| added_steps.contains(&step.id))
        .cloned()
        .collect::<Vec<_>>();
    let applied_edges = applied_steps
        .iter()
        .flat_map(|step| {
            step.needs
                .iter()
                .map(|need| (need.clone(), step.id.clone()))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let updated_steps = updated_steps.into_iter().collect::<Vec<_>>();
    let invalidated_steps = impacted_steps(&updated_steps, &flow.edges);
    ensure_can_invalidate_steps(&original_steps, &invalidated_steps)?;

    persist_added_steps(store, &patch.flow_id, &applied_steps)?;
    persist_param_updates(store, &patch.flow_id, &draft.steps, &updated_steps)?;
    invalidate_steps(store, &patch.flow_id, &original_steps, &invalidated_steps)?;

    Ok(GraphPatchApplication {
        patch_id: patch.id.clone(),
        flow_id: patch.flow_id.clone(),
        applied_steps: applied_steps
            .iter()
            .map(|step| step.id.clone())
            .collect::<Vec<_>>(),
        applied_edges,
        updated_steps,
        invalidated_steps,
    })
}

fn impacted_steps(
    updated_steps: &[String],
    edges: &[crate::storage::StoredFlowEdge],
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut impacted = Vec::new();
    let mut queue = updated_steps.to_vec();
    let mut index = 0;
    while index < queue.len() {
        let step_id = queue[index].clone();
        index += 1;
        if !seen.insert(step_id.clone()) {
            continue;
        }
        impacted.push(step_id.clone());
        for edge in edges
            .iter()
            .filter(|edge| edge.edge_type == "needs" && edge.from_local_id == step_id)
        {
            queue.push(edge.to_local_id.clone());
        }
    }
    impacted
}

fn ensure_can_invalidate_steps(
    original_steps: &BTreeMap<String, StoredFlowStep>,
    invalidated_steps: &[String],
) -> Result<(), StorageError> {
    for step_id in invalidated_steps {
        let Some(step) = original_steps.get(step_id) else {
            continue;
        };
        if step.status == StepStatus::Running.as_str() {
            return Err(StorageError::InvalidInput(format!(
                "graph patch cannot invalidate running step {step_id}"
            )));
        }
    }
    Ok(())
}

fn flow_inspection_to_draft(
    flow: &crate::storage::FlowInspection,
) -> Result<FlowDraft, StorageError> {
    let mut needs_by_step = flow
        .steps
        .iter()
        .map(|step| (step.local_id.clone(), Vec::<String>::new()))
        .collect::<BTreeMap<_, _>>();
    for edge in &flow.edges {
        if edge.edge_type == "needs" {
            needs_by_step
                .entry(edge.to_local_id.clone())
                .or_default()
                .push(edge.from_local_id.clone());
        }
    }

    let steps = flow
        .steps
        .iter()
        .map(|step| {
            Ok(FlowStepDraft {
                id: step.local_id.clone(),
                tool_ref: step.tool_ref.clone().unwrap_or_default(),
                needs: needs_by_step.remove(&step.local_id).unwrap_or_default(),
                reason: step.reason.clone(),
                inputs: parse_json_map(&step.inputs_json)?,
                params: parse_json_map(&step.params_json)?,
                outputs: parse_json_map(&step.outputs_json)?,
            })
        })
        .collect::<Result<Vec<_>, StorageError>>()?;

    Ok(FlowDraft {
        schema_version: flow.schema_version.clone(),
        id: flow.id.clone(),
        name: flow.name.clone(),
        steps,
        source_text: String::new(),
    })
}

fn persist_added_steps(
    store: &ProjectStore,
    flow_id: &str,
    steps: &[FlowStepDraft],
) -> Result<(), StorageError> {
    let now = crate::storage::now_unix_seconds();
    for step in steps {
        store.connection().execute(
            "INSERT INTO steps
             (id, flow_id, tool_ref, type, status, reason, params_json, inputs_json, outputs_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'analysis', ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                db_step_id(flow_id, &step.id),
                flow_id,
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

        for need in &step.needs {
            store.connection().execute(
                "INSERT INTO edges (id, flow_id, from_step_id, to_step_id, edge_type)
                 VALUES (?1, ?2, ?3, ?4, 'needs')",
                params![
                    edge_id(flow_id, need, &step.id),
                    flow_id,
                    db_step_id(flow_id, need),
                    db_step_id(flow_id, &step.id)
                ],
            )?;
        }
    }

    store.connection().execute(
        "UPDATE flows SET updated_at = ?1 WHERE id = ?2",
        params![now, flow_id],
    )?;
    Ok(())
}

fn persist_param_updates(
    store: &ProjectStore,
    flow_id: &str,
    draft_steps: &[FlowStepDraft],
    updated_steps: &[String],
) -> Result<(), StorageError> {
    if updated_steps.is_empty() {
        return Ok(());
    }
    let now = crate::storage::now_unix_seconds();
    for step_id in updated_steps {
        let step = draft_steps
            .iter()
            .find(|step| &step.id == step_id)
            .expect("updated step came from draft");
        store.connection().execute(
            "UPDATE steps SET params_json = ?1, updated_at = ?2 WHERE id = ?3 AND flow_id = ?4",
            params![
                map_json(&step.params),
                now,
                db_step_id(flow_id, step_id),
                flow_id
            ],
        )?;
    }
    store.connection().execute(
        "UPDATE flows SET updated_at = ?1 WHERE id = ?2",
        params![now, flow_id],
    )?;
    Ok(())
}

fn invalidate_steps(
    store: &ProjectStore,
    flow_id: &str,
    original_steps: &BTreeMap<String, StoredFlowStep>,
    invalidated_steps: &[String],
) -> Result<(), StorageError> {
    if invalidated_steps.is_empty() {
        return Ok(());
    }
    let now = crate::storage::now_unix_seconds();
    for step_id in invalidated_steps {
        if !original_steps.contains_key(step_id) {
            continue;
        }
        store.connection().execute(
            "UPDATE steps SET status = ?1, updated_at = ?2 WHERE id = ?3 AND flow_id = ?4",
            params![
                StepStatus::Draft.as_str(),
                now,
                db_step_id(flow_id, step_id),
                flow_id
            ],
        )?;
    }
    store.connection().execute(
        "UPDATE flows SET updated_at = ?1 WHERE id = ?2",
        params![now, flow_id],
    )?;
    Ok(())
}

fn parse_graph_patch_operations(
    patch_json: &str,
) -> Result<Vec<GraphPatchOperation>, StorageError> {
    let root = JsonParser::new(patch_json).parse()?;
    let root = root.as_object().ok_or_else(|| {
        StorageError::InvalidInput("graph patch JSON must be an object".to_string())
    })?;
    let ops = root
        .get("ops")
        .ok_or_else(|| StorageError::InvalidInput("graph patch JSON is missing ops".to_string()))?
        .as_array()
        .ok_or_else(|| {
            StorageError::InvalidInput("graph patch ops must be an array".to_string())
        })?;

    ops.iter()
        .map(|op| {
            let op = op.as_object().ok_or_else(|| {
                StorageError::InvalidInput("graph patch operation must be an object".to_string())
            })?;
            let op_name = required_json_string(op, "op")?;
            match op_name.as_str() {
                "add_step" => Ok(GraphPatchOperation::AddStep(FlowStepDraft {
                    id: required_json_string(op, "id")?,
                    tool_ref: required_json_string(op, "tool")?,
                    needs: optional_json_string_array(op, "needs")?,
                    reason: optional_json_string(op, "reason")?,
                    inputs: optional_json_string_map(op, "inputs")?,
                    params: optional_json_string_map(op, "params")?,
                    outputs: optional_json_string_map(op, "outputs")?,
                })),
                "add_edge" => Ok(GraphPatchOperation::AddEdge {
                    from: required_json_string(op, "from")?,
                    to: required_json_string(op, "to")?,
                }),
                "update_params" => Ok(GraphPatchOperation::UpdateParams {
                    step: required_json_string(op, "step")?,
                    params: required_json_string_map(op, "params")?,
                }),
                other => Err(StorageError::InvalidInput(format!(
                    "unsupported graph patch op {other}"
                ))),
            }
        })
        .collect()
}

fn required_json_string(
    object: &BTreeMap<String, JsonValue>,
    field: &str,
) -> Result<String, StorageError> {
    object
        .get(field)
        .and_then(JsonValue::as_string)
        .map(str::to_string)
        .ok_or_else(|| {
            StorageError::InvalidInput(format!("graph patch operation is missing string {field}"))
        })
}

fn optional_json_string(
    object: &BTreeMap<String, JsonValue>,
    field: &str,
) -> Result<Option<String>, StorageError> {
    object
        .get(field)
        .map(|value| {
            value.as_string().map(str::to_string).ok_or_else(|| {
                StorageError::InvalidInput(format!("graph patch field {field} must be a string"))
            })
        })
        .transpose()
}

fn optional_json_string_array(
    object: &BTreeMap<String, JsonValue>,
    field: &str,
) -> Result<Vec<String>, StorageError> {
    let Some(value) = object.get(field) else {
        return Ok(Vec::new());
    };
    let values = value.as_array().ok_or_else(|| {
        StorageError::InvalidInput(format!("graph patch field {field} must be an array"))
    })?;
    values
        .iter()
        .map(|value| {
            value.as_string().map(str::to_string).ok_or_else(|| {
                StorageError::InvalidInput(format!(
                    "graph patch field {field} must contain only strings"
                ))
            })
        })
        .collect()
}

fn optional_json_string_map(
    object: &BTreeMap<String, JsonValue>,
    field: &str,
) -> Result<BTreeMap<String, String>, StorageError> {
    let Some(value) = object.get(field) else {
        return Ok(BTreeMap::new());
    };
    let map = value.as_object().ok_or_else(|| {
        StorageError::InvalidInput(format!("graph patch field {field} must be an object"))
    })?;
    map.iter()
        .map(|(key, value)| {
            let value = value.as_string().ok_or_else(|| {
                StorageError::InvalidInput(format!(
                    "graph patch field {field}.{key} must be a string"
                ))
            })?;
            Ok((key.clone(), value.to_string()))
        })
        .collect()
}

fn required_json_string_map(
    object: &BTreeMap<String, JsonValue>,
    field: &str,
) -> Result<BTreeMap<String, String>, StorageError> {
    if !object.contains_key(field) {
        return Err(StorageError::InvalidInput(format!(
            "graph patch operation is missing object {field}"
        )));
    }
    optional_json_string_map(object, field)
}

fn application_payload_json(application: &GraphPatchApplication) -> String {
    let steps = application
        .applied_steps
        .iter()
        .map(|step| format!("\"{}\"", escape_json(step)))
        .collect::<Vec<_>>()
        .join(",");
    let edges = application
        .applied_edges
        .iter()
        .map(|(from, to)| {
            format!(
                "{{\"from\":\"{}\",\"to\":\"{}\"}}",
                escape_json(from),
                escape_json(to)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let updated_steps = application
        .updated_steps
        .iter()
        .map(|step| format!("\"{}\"", escape_json(step)))
        .collect::<Vec<_>>()
        .join(",");
    let invalidated_steps = application
        .invalidated_steps
        .iter()
        .map(|step| format!("\"{}\"", escape_json(step)))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"patch_id\":\"{}\",\"applied_steps\":[{}],\"applied_edges\":[{}],\"updated_steps\":[{}],\"invalidated_steps\":[{}]}}",
        escape_json(&application.patch_id),
        steps,
        edges,
        updated_steps,
        invalidated_steps
    )
}

fn parse_json_map(input: &str) -> Result<BTreeMap<String, String>, StorageError> {
    let root = JsonParser::new(input).parse()?;
    let object = root
        .as_object()
        .ok_or_else(|| StorageError::InvalidInput("expected JSON object".to_string()))?;
    object
        .iter()
        .map(|(key, value)| {
            let value = value.as_string().ok_or_else(|| {
                StorageError::InvalidInput(format!("JSON map field {key} must be a string"))
            })?;
            Ok((key.clone(), value.to_string()))
        })
        .collect()
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

fn edge_id(flow_id: &str, from_step_id: &str, to_step_id: &str) -> String {
    format!("edge:{flow_id}/{from_step_id}->{to_step_id}")
}

fn proposal_payload_json(title: &str, reason: &str, patch_json: &str) -> String {
    format!(
        concat!(
            "{{",
            "\"title\":\"{}\",",
            "\"reason\":\"{}\",",
            "\"patch_json\":\"{}\"",
            "}}"
        ),
        escape_json(title),
        escape_json(reason),
        escape_json(patch_json),
    )
}

fn decision_payload_json(patch_id: &str, reason: Option<&str>) -> String {
    match reason {
        Some(reason) => format!(
            "{{\"patch_id\":\"{}\",\"reason\":\"{}\"}}",
            escape_json(patch_id),
            escape_json(reason),
        ),
        None => format!("{{\"patch_id\":\"{}\"}}", escape_json(patch_id)),
    }
}

fn validate_flow_id(flow_id: &str) -> Result<&str, StorageError> {
    validate_non_empty("flow id", flow_id)
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

fn validate_non_empty<'a>(label: &str, value: &'a str) -> Result<&'a str, StorageError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(StorageError::InvalidInput(format!(
            "{label} must not be empty"
        )))
    } else {
        Ok(trimmed)
    }
}

fn json_string_field(json: &str, field: &str) -> Option<String> {
    let needle = format!("\"{field}\":\"");
    let start = json.find(&needle)? + needle.len();
    let rest = &json[start..];
    let end = find_json_string_end(rest)?;
    Some(unescape_json_string(&rest[..end]))
}

fn find_json_string_end(input: &str) -> Option<usize> {
    let mut escaped = false;
    for (index, ch) in input.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Some(index),
            _ => {}
        }
    }
    None
}

fn unescape_json_string(input: &str) -> String {
    let mut output = String::new();
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        match chars.next() {
            Some('"') => output.push('"'),
            Some('\\') => output.push('\\'),
            Some('n') => output.push('\n'),
            Some('r') => output.push('\r'),
            Some('t') => output.push('\t'),
            Some('u') => {
                let digits = chars.by_ref().take(4).collect::<String>();
                if let Ok(code) = u32::from_str_radix(&digits, 16) {
                    if let Some(decoded) = char::from_u32(code) {
                        output.push(decoded);
                    }
                }
            }
            Some(other) => output.push(other),
            None => break,
        }
    }
    output
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
            ch if ch.is_control() => output.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => output.push(ch),
        }
    }
    output
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum JsonValue {
    Object(BTreeMap<String, JsonValue>),
    Array(Vec<JsonValue>),
    String(String),
    Null,
}

impl JsonValue {
    fn as_object(&self) -> Option<&BTreeMap<String, JsonValue>> {
        match self {
            Self::Object(value) => Some(value),
            _ => None,
        }
    }

    fn as_array(&self) -> Option<&[JsonValue]> {
        match self {
            Self::Array(value) => Some(value),
            _ => None,
        }
    }

    fn as_string(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }
}

struct JsonParser<'a> {
    input: &'a str,
    index: usize,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, index: 0 }
    }

    fn parse(mut self) -> Result<JsonValue, StorageError> {
        let value = self.parse_value()?;
        self.skip_whitespace();
        if self.index == self.input.len() {
            Ok(value)
        } else {
            Err(StorageError::InvalidInput(format!(
                "unexpected trailing JSON in graph patch: {}",
                self.input
            )))
        }
    }

    fn parse_value(&mut self) -> Result<JsonValue, StorageError> {
        self.skip_whitespace();
        match self.peek_char() {
            Some('{') => self.parse_object(),
            Some('[') => self.parse_array(),
            Some('"') => self.parse_string().map(JsonValue::String),
            Some('n') => self.parse_null(),
            Some(other) => Err(StorageError::InvalidInput(format!(
                "unsupported JSON value starting with {other}"
            ))),
            None => Err(StorageError::InvalidInput(
                "unexpected end of JSON".to_string(),
            )),
        }
    }

    fn parse_object(&mut self) -> Result<JsonValue, StorageError> {
        self.expect_char('{')?;
        let mut object = BTreeMap::new();
        self.skip_whitespace();
        if self.consume_char('}') {
            return Ok(JsonValue::Object(object));
        }

        loop {
            self.skip_whitespace();
            let key = self.parse_string()?;
            self.skip_whitespace();
            self.expect_char(':')?;
            let value = self.parse_value()?;
            object.insert(key, value);
            self.skip_whitespace();
            if self.consume_char(',') {
                continue;
            }
            self.expect_char('}')?;
            break;
        }

        Ok(JsonValue::Object(object))
    }

    fn parse_array(&mut self) -> Result<JsonValue, StorageError> {
        self.expect_char('[')?;
        let mut values = Vec::new();
        self.skip_whitespace();
        if self.consume_char(']') {
            return Ok(JsonValue::Array(values));
        }

        loop {
            values.push(self.parse_value()?);
            self.skip_whitespace();
            if self.consume_char(',') {
                continue;
            }
            self.expect_char(']')?;
            break;
        }

        Ok(JsonValue::Array(values))
    }

    fn parse_string(&mut self) -> Result<String, StorageError> {
        self.expect_char('"')?;
        let mut output = String::new();
        while let Some(ch) = self.next_char() {
            match ch {
                '"' => return Ok(output),
                '\\' => output.push(self.parse_escape()?),
                ch => output.push(ch),
            }
        }
        Err(StorageError::InvalidInput(
            "unterminated JSON string".to_string(),
        ))
    }

    fn parse_escape(&mut self) -> Result<char, StorageError> {
        match self.next_char() {
            Some('"') => Ok('"'),
            Some('\\') => Ok('\\'),
            Some('/') => Ok('/'),
            Some('n') => Ok('\n'),
            Some('r') => Ok('\r'),
            Some('t') => Ok('\t'),
            Some('b') => Ok('\u{0008}'),
            Some('f') => Ok('\u{000c}'),
            Some('u') => {
                let mut code = String::new();
                for _ in 0..4 {
                    code.push(self.next_char().ok_or_else(|| {
                        StorageError::InvalidInput("incomplete JSON unicode escape".to_string())
                    })?);
                }
                let value = u32::from_str_radix(&code, 16).map_err(|_| {
                    StorageError::InvalidInput(format!("invalid JSON unicode escape {code}"))
                })?;
                char::from_u32(value).ok_or_else(|| {
                    StorageError::InvalidInput(format!("invalid JSON unicode scalar {code}"))
                })
            }
            Some(other) => Err(StorageError::InvalidInput(format!(
                "unsupported JSON escape {other}"
            ))),
            None => Err(StorageError::InvalidInput(
                "incomplete JSON escape".to_string(),
            )),
        }
    }

    fn parse_null(&mut self) -> Result<JsonValue, StorageError> {
        if self.input[self.index..].starts_with("null") {
            self.index += 4;
            Ok(JsonValue::Null)
        } else {
            Err(StorageError::InvalidInput("invalid JSON null".to_string()))
        }
    }

    fn expect_char(&mut self, expected: char) -> Result<(), StorageError> {
        if self.consume_char(expected) {
            Ok(())
        } else {
            Err(StorageError::InvalidInput(format!(
                "expected JSON char {expected}"
            )))
        }
    }

    fn consume_char(&mut self, expected: char) -> bool {
        if self.peek_char().is_some_and(|actual| actual == expected) {
            self.index += expected.len_utf8();
            true
        } else {
            false
        }
    }

    fn skip_whitespace(&mut self) {
        while self.peek_char().is_some_and(char::is_whitespace) {
            let ch = self.peek_char().expect("checked above");
            self.index += ch.len_utf8();
        }
    }

    fn peek_char(&self) -> Option<char> {
        self.input.get(self.index..)?.chars().next()
    }

    fn next_char(&mut self) -> Option<char> {
        let ch = self.peek_char()?;
        self.index += ch.len_utf8();
        Some(ch)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use rusqlite::params;

    use crate::storage::{
        now_unix_seconds, ArtifactImportMode, ArtifactImportRequest, FlowDraft, ProjectStore,
        StorageError, ToolSpec,
    };

    fn temp_project_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentflow-graph-patch-{test_name}-{}-{}",
            std::process::id(),
            now_unix_seconds()
        ))
    }

    fn seed_flow(store: &ProjectStore, flow_id: &str) {
        let now = now_unix_seconds();
        store
            .connection()
            .execute(
                "INSERT INTO flows
                 (id, name, status, source_path, schema_version, created_at, updated_at)
                 VALUES (?1, ?2, 'approved', NULL, ?3, ?4, ?5)",
                params![
                    flow_id,
                    format!("Flow {flow_id}"),
                    agentflow_schemas::FLOW_SCHEMA_V0,
                    now,
                    now
                ],
            )
            .unwrap();
    }

    fn setup_apply_store(test_name: &str) -> (ProjectStore, PathBuf, String, String) {
        let path = temp_project_path(test_name);
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Graph Patch Apply")).unwrap();
        store
            .register_tool(
                ToolSpec::from_simple_yaml(
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
"#,
                )
                .unwrap(),
            )
            .unwrap();
        let expression_path = path.join("expression.tsv");
        fs::write(&expression_path, "sample\tTP53\nA\t1.0\n").unwrap();
        let expression_id = store
            .import_artifact(ArtifactImportRequest {
                source_path: expression_path,
                artifact_type: "TSV".to_string(),
                mode: ArtifactImportMode::Reference,
            })
            .unwrap()
            .summary
            .id;
        let survival_path = path.join("survival.tsv");
        fs::write(&survival_path, "sample\ttime\tstatus\nA\t10\t1\n").unwrap();
        let survival_id = store
            .import_artifact(ArtifactImportRequest {
                source_path: survival_path,
                artifact_type: "TSV".to_string(),
                mode: ArtifactImportMode::Reference,
            })
            .unwrap()
            .summary
            .id;
        store
            .approve_flow(
                FlowDraft::from_simple_yaml(&format!(
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
      expression_table: {expression_id}
      survival_table: {survival_id}
    params:
      gene: TP53
    outputs:
      report: marker_report
"#
                ))
                .unwrap(),
                None,
            )
            .unwrap();
        (store, path, expression_id, survival_id)
    }

    #[test]
    fn proposes_lists_and_approves_graph_patches() {
        let path = temp_project_path("approve");
        let store = ProjectStore::init(&path, Some("Graph Patch Demo")).unwrap();
        seed_flow(&store, "flow_alpha");

        let patch = store
            .propose_graph_patch(
                "flow_alpha",
                "Add QC branch",
                "Capture an explicit review gate before publish.",
                r#"{"ops":[{"op":"add_edge","from":"review","to":"publish"}]}"#,
            )
            .unwrap();

        assert_eq!(patch.flow_id, "flow_alpha");
        assert_eq!(patch.status, "pending");
        assert!(patch.decision_reason.is_none());
        assert_eq!(
            patch.patch_json,
            r#"{"ops":[{"op":"add_edge","from":"review","to":"publish"}]}"#
        );

        let listed = store.list_graph_patches("flow_alpha").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "Add QC branch");
        assert_eq!(listed[0].status, "pending");

        let approved = store.approve_graph_patch(&patch.id).unwrap();
        assert_eq!(approved.id, patch.id);
        assert_eq!(approved.status, "approved");
        assert!(approved.decided_at.is_some());

        let statuses = store.list_graph_patches("flow_alpha").unwrap();
        assert_eq!(statuses[0].status, "approved");

        let mut stmt = store
            .connection()
            .prepare(
                "SELECT event_type FROM events
                 WHERE flow_id = ?1
                 ORDER BY created_at ASC, id ASC",
            )
            .unwrap();
        let rows = stmt
            .query_map(params!["flow_alpha"], |row| row.get::<_, String>(0))
            .unwrap();
        let event_types = rows.map(Result::unwrap).collect::<Vec<_>>();
        assert_eq!(
            event_types,
            vec![
                super::GRAPH_PATCH_PROPOSED_EVENT.to_string(),
                super::GRAPH_PATCH_APPROVED_EVENT.to_string()
            ]
        );

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn rejects_graph_patches_and_blocks_second_decision() {
        let path = temp_project_path("reject");
        let store = ProjectStore::init(&path, Some("Graph Patch Demo")).unwrap();
        seed_flow(&store, "flow_beta");

        let patch = store
            .propose_graph_patch(
                "flow_beta",
                "Remove loopback edge",
                "The proposal introduces a cycle in review.",
                r#"{"ops":[{"op":"remove_edge","from":"publish","to":"draft"}]}"#,
            )
            .unwrap();

        let rejected = store
            .reject_graph_patch(&patch.id, "Would bypass the existing approval gate.")
            .unwrap();
        assert_eq!(rejected.status, "rejected");
        assert_eq!(
            rejected.decision_reason.as_deref(),
            Some("Would bypass the existing approval gate.")
        );

        let error = store.approve_graph_patch(&patch.id).unwrap_err();
        assert!(matches!(error, StorageError::InvalidInput(_)));
        assert!(error.to_string().contains("has already been rejected"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn applies_approved_add_step_patch_to_flow_graph() {
        let (store, path, expression_id, survival_id) = setup_apply_store("apply-add-step");
        let patch = store
            .propose_graph_patch(
                "marker_demo",
                "Add ortholog scan",
                "Primary marker was weak, so branch into a related candidate.",
                &format!(
                    r#"{{
  "ops": [
    {{
      "op": "add_step",
      "id": "ortholog_scan",
      "tool": "marker/marker_survival_scan",
      "reason": "Evaluate related marker signal",
      "needs": ["scan"],
      "inputs": {{
        "expression_table": "{expression_id}",
        "survival_table": "{survival_id}"
      }},
      "params": {{
        "gene": "EGFR"
      }},
      "outputs": {{
        "report": "ortholog_report"
      }}
    }}
  ]
}}"#
                ),
            )
            .unwrap();

        store.approve_graph_patch(&patch.id).unwrap();
        let application = store.apply_graph_patch(&patch.id).unwrap();
        assert_eq!(application.applied_steps, vec!["ortholog_scan"]);
        assert_eq!(
            application.applied_edges,
            vec![("scan".to_string(), "ortholog_scan".to_string())]
        );

        let flow = store.inspect_flow("marker_demo").unwrap();
        assert_eq!(flow.steps.len(), 2);
        assert!(flow
            .steps
            .iter()
            .any(|step| step.local_id == "ortholog_scan"));
        assert!(flow
            .edges
            .iter()
            .any(|edge| edge.from_local_id == "scan" && edge.to_local_id == "ortholog_scan"));
        assert_eq!(
            store.inspect_graph_patch(&patch.id).unwrap().status,
            "applied"
        );

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn update_params_patch_updates_step_and_invalidates_downstream() {
        let (store, path, expression_id, survival_id) =
            setup_apply_store("update-params-invalidates");
        let add_patch = store
            .propose_graph_patch(
                "marker_demo",
                "Add downstream branch",
                "Create a downstream step that must rerun when the parent changes.",
                &format!(
                    r#"{{"ops":[{{"op":"add_step","id":"ortholog_scan","tool":"marker/marker_survival_scan","needs":["scan"],"inputs":{{"expression_table":"{expression_id}","survival_table":"{survival_id}"}},"params":{{"gene":"EGFR"}},"outputs":{{"report":"ortholog_report"}}}}]}}"#
                ),
            )
            .unwrap();
        store.approve_graph_patch(&add_patch.id).unwrap();
        store.apply_graph_patch(&add_patch.id).unwrap();
        store
            .connection()
            .execute(
                "UPDATE steps SET status = 'completed' WHERE flow_id = ?1",
                params!["marker_demo"],
            )
            .unwrap();

        let patch = store
            .propose_graph_patch(
                "marker_demo",
                "Retest marker",
                "TP53 was weak, so replay the branch with ALK.",
                r#"{"ops":[{"op":"update_params","step":"scan","params":{"gene":"ALK"}}]}"#,
            )
            .unwrap();
        store.approve_graph_patch(&patch.id).unwrap();
        let application = store.apply_graph_patch(&patch.id).unwrap();

        assert_eq!(application.updated_steps, vec!["scan"]);
        assert_eq!(application.invalidated_steps, vec!["scan", "ortholog_scan"]);

        let flow = store.inspect_flow("marker_demo").unwrap();
        let scan = flow
            .steps
            .iter()
            .find(|step| step.local_id == "scan")
            .unwrap();
        let downstream = flow
            .steps
            .iter()
            .find(|step| step.local_id == "ortholog_scan")
            .unwrap();
        assert_eq!(scan.params_json, r#"{"gene":"ALK"}"#);
        assert_eq!(scan.status, crate::domain::StepStatus::Draft.as_str());
        assert_eq!(downstream.status, crate::domain::StepStatus::Draft.as_str());

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn update_params_patch_revalidates_tool_contract() {
        let (store, path, _expression_id, _survival_id) =
            setup_apply_store("update-params-invalid");
        let patch = store
            .propose_graph_patch(
                "marker_demo",
                "Add invalid param",
                "The patch should be checked against the registered tool.",
                r#"{"ops":[{"op":"update_params","step":"scan","params":{"unknown":"x"}}]}"#,
            )
            .unwrap();
        store.approve_graph_patch(&patch.id).unwrap();
        let error = store.apply_graph_patch(&patch.id).unwrap_err();
        assert!(error.to_string().contains("unknown param unknown"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn update_params_patch_rejects_running_invalidation_without_partial_update() {
        let (store, path, _expression_id, _survival_id) =
            setup_apply_store("update-params-running");
        store
            .connection()
            .execute(
                "UPDATE steps SET status = 'running' WHERE flow_id = ?1 AND id = ?2",
                params!["marker_demo", "step:marker_demo/scan"],
            )
            .unwrap();

        let patch = store
            .propose_graph_patch(
                "marker_demo",
                "Retest marker",
                "Running work must not be rewritten.",
                r#"{"ops":[{"op":"update_params","step":"scan","params":{"gene":"ALK"}}]}"#,
            )
            .unwrap();
        store.approve_graph_patch(&patch.id).unwrap();
        let error = store.apply_graph_patch(&patch.id).unwrap_err();
        assert!(error.to_string().contains("cannot invalidate running step"));

        let flow = store.inspect_flow("marker_demo").unwrap();
        let scan = flow
            .steps
            .iter()
            .find(|step| step.local_id == "scan")
            .unwrap();
        assert_eq!(scan.params_json, r#"{"gene":"TP53"}"#);
        assert_eq!(scan.status, "running");

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn apply_requires_approval_and_blocks_reapply() {
        let (store, path, expression_id, survival_id) =
            setup_apply_store("apply-requires-approval");
        let patch = store
            .propose_graph_patch(
                "marker_demo",
                "Add branch",
                "Try a related candidate.",
                &format!(
                    r#"{{"ops":[{{"op":"add_step","id":"branch_scan","tool":"marker/marker_survival_scan","needs":["scan"],"inputs":{{"expression_table":"{expression_id}","survival_table":"{survival_id}"}},"params":{{"gene":"ALK"}},"outputs":{{"report":"branch_report"}}}}]}}"#
                ),
            )
            .unwrap();

        let error = store.apply_graph_patch(&patch.id).unwrap_err();
        assert!(error.to_string().contains("must be approved"));

        store.approve_graph_patch(&patch.id).unwrap();
        store.apply_graph_patch(&patch.id).unwrap();
        let error = store.apply_graph_patch(&patch.id).unwrap_err();
        assert!(error.to_string().contains("current status is applied"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn add_edge_can_only_target_new_steps() {
        let (store, path, _expression_id, _survival_id) = setup_apply_store("edge-target");
        let patch = store
            .propose_graph_patch(
                "marker_demo",
                "Touch existing dependency",
                "Existing completed steps should not be rewired by this primitive.",
                r#"{"ops":[{"op":"add_edge","from":"scan","to":"scan"}]}"#,
            )
            .unwrap();

        store.approve_graph_patch(&patch.id).unwrap();
        let error = store.apply_graph_patch(&patch.id).unwrap_err();
        assert!(error
            .to_string()
            .contains("can only target newly added steps"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn graph_patch_requires_existing_flow() {
        let path = temp_project_path("missing-flow");
        let store = ProjectStore::init(&path, Some("Graph Patch Demo")).unwrap();

        let error = store
            .propose_graph_patch(
                "unknown_flow",
                "Add edge",
                "Reason",
                r#"{"ops":[{"op":"add_edge"}]}"#,
            )
            .unwrap_err();

        assert!(matches!(error, StorageError::NotFound(_)));
        assert!(error.to_string().contains("flow unknown_flow"));

        let _ = fs::remove_dir_all(path);
    }
}
