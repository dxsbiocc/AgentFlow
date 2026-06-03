use std::fmt;

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::storage::{EventRecord, ProjectStore, StorageError};

const DECISION_POINT_RAISED_EVENT: &str = "handoff.decision_point_raised";
const USER_RESOLVED_EVENT: &str = "handoff.user_resolved";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cost {
    Cheap,
    Moderate,
    Expensive,
}

impl Cost {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cheap => "cheap",
            Self::Moderate => "moderate",
            Self::Expensive => "expensive",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "cheap" => Some(Self::Cheap),
            "moderate" => Some(Self::Moderate),
            "expensive" => Some(Self::Expensive),
            _ => None,
        }
    }
}

impl fmt::Display for Cost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    Low,
    Medium,
    High,
}

impl Risk {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

impl fmt::Display for Risk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    DeepenOrStop,
    PremiseChallenged,
    BudgetThreshold,
    GoalMutation,
    ToolGap,
    StanceAssessment,
}

impl DecisionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DeepenOrStop => "deepen_or_stop",
            Self::PremiseChallenged => "premise_challenged",
            Self::BudgetThreshold => "budget_threshold",
            Self::GoalMutation => "goal_mutation",
            Self::ToolGap => "tool_gap",
            Self::StanceAssessment => "stance_assessment",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "deepen_or_stop" => Some(Self::DeepenOrStop),
            "premise_challenged" => Some(Self::PremiseChallenged),
            "budget_threshold" => Some(Self::BudgetThreshold),
            "goal_mutation" => Some(Self::GoalMutation),
            "tool_gap" => Some(Self::ToolGap),
            "stance_assessment" => Some(Self::StanceAssessment),
            _ => None,
        }
    }
}

impl fmt::Display for DecisionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskClass {
    Labor,
    Decision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffOption {
    pub label: String,
    pub direction: String,
    pub cost: Cost,
    pub risk: Risk,
    pub reversible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionPoint {
    pub id: String,
    pub kind: DecisionKind,
    pub digest: String,
    pub options: Vec<HandoffOption>,
    pub recommendation: usize,
    pub status: DecisionStatus,
    pub resolution: Option<Resolution>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    Pending,
    Resolved,
}

impl DecisionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Resolved => "resolved",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "pending" => Some(Self::Pending),
            "resolved" => Some(Self::Resolved),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    pub chosen_index: usize,
    pub note: String,
    pub resolved_at: i64,
}

impl DecisionPoint {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("decision point serializes to JSON")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepContext {
    pub cost: Cost,
    pub reversible: bool,
    pub equivalent_branches: bool,
    pub conflicts_user_premise: bool,
    pub mutates_goal: bool,
    pub near_budget: bool,
}

pub trait InterventionPolicy {
    fn assess(&self, ctx: &StepContext) -> Option<DecisionKind>;
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DefaultPolicy;

impl InterventionPolicy for DefaultPolicy {
    fn assess(&self, ctx: &StepContext) -> Option<DecisionKind> {
        if ctx.mutates_goal {
            Some(DecisionKind::GoalMutation)
        } else if ctx.conflicts_user_premise {
            Some(DecisionKind::PremiseChallenged)
        } else if ctx.near_budget {
            Some(DecisionKind::BudgetThreshold)
        } else if matches!(ctx.cost, Cost::Expensive) || !ctx.reversible || ctx.equivalent_branches
        {
            Some(DecisionKind::DeepenOrStop)
        } else {
            None
        }
    }
}

pub fn classify(consequential: bool, user_cares: bool) -> TaskClass {
    if consequential && user_cares {
        TaskClass::Decision
    } else {
        TaskClass::Labor
    }
}

impl ProjectStore {
    pub fn raise_decision_point(
        &self,
        kind: DecisionKind,
        digest: &str,
        options: Vec<HandoffOption>,
        recommendation: usize,
    ) -> Result<DecisionPoint, StorageError> {
        let digest = validate_raise_input(digest, &options, recommendation)?;
        let id = self.append_event(EventRecord {
            flow_id: None,
            step_id: None,
            run_id: None,
            event_type: DECISION_POINT_RAISED_EVENT.to_string(),
            payload_json: decision_point_payload_json(kind, digest, &options, recommendation),
        })?;
        self.touch_project()?;
        self.inspect_decision_point(&id)
    }

    pub fn list_decision_points(&self) -> Result<Vec<DecisionPoint>, StorageError> {
        let reverted = self.reverted_event_id_set()?;
        let mut stmt = self.connection().prepare(
            "SELECT id, event_type, payload_json, created_at
             FROM events
             WHERE event_type IN (?1, ?2)
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map(
            params![DECISION_POINT_RAISED_EVENT, USER_RESOLVED_EVENT],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )?;

        let mut points = Vec::new();
        for row in rows {
            let (event_id, event_type, payload_json, created_at) = row?;
            if reverted.contains(&event_id) {
                continue;
            }
            match event_type.as_str() {
                DECISION_POINT_RAISED_EVENT => {
                    points.push(decision_point_from_raised_event(
                        event_id,
                        &payload_json,
                        created_at,
                    )?);
                }
                USER_RESOLVED_EVENT => {
                    let payload = resolution_payload_from_json(&event_id, &payload_json)?;
                    if let Some(point) = points
                        .iter_mut()
                        .find(|point: &&mut DecisionPoint| point.id == payload.decision_point_id)
                    {
                        apply_resolution(point, &event_id, payload, created_at)?;
                    }
                }
                _ => {}
            }
        }
        Ok(points)
    }

    pub fn pending_decision_points(&self) -> Result<Vec<DecisionPoint>, StorageError> {
        Ok(self
            .list_decision_points()?
            .into_iter()
            .filter(|point| point.status == DecisionStatus::Pending)
            .collect())
    }

    pub fn inspect_decision_point(&self, id: &str) -> Result<DecisionPoint, StorageError> {
        let id = validate_non_empty("decision point id", id)?;
        self.list_decision_points()?
            .into_iter()
            .find(|point| point.id == id)
            .ok_or_else(|| StorageError::NotFound(format!("decision point {id}")))
    }

    pub fn resolve_decision_point(
        &self,
        id: &str,
        chosen_index: usize,
        note: &str,
    ) -> Result<DecisionPoint, StorageError> {
        let id = validate_non_empty("decision point id", id)?;
        let point = self.inspect_decision_point(id)?;
        if point.status != DecisionStatus::Pending {
            return Err(StorageError::InvalidInput(format!(
                "decision point {id} has already been resolved"
            )));
        }
        if chosen_index >= point.options.len() {
            return Err(StorageError::InvalidInput(format!(
                "decision point {id} chosen_index must be a valid option index"
            )));
        }

        self.append_event(EventRecord {
            flow_id: None,
            step_id: None,
            run_id: None,
            event_type: USER_RESOLVED_EVENT.to_string(),
            payload_json: resolution_payload_json(id, chosen_index, note),
        })?;
        self.touch_project()?;
        self.inspect_decision_point(id)
    }
}

fn validate_raise_input<'a>(
    digest: &'a str,
    options: &[HandoffOption],
    recommendation: usize,
) -> Result<&'a str, StorageError> {
    let digest = validate_non_empty("decision point digest", digest)?;
    if options.is_empty() {
        return Err(StorageError::InvalidInput(
            "decision point options must not be empty".to_string(),
        ));
    }
    if recommendation >= options.len() {
        return Err(StorageError::InvalidInput(
            "decision point recommendation must be a valid option index".to_string(),
        ));
    }
    Ok(digest)
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

#[derive(Debug, Serialize, Deserialize)]
struct DecisionPointPayload {
    kind: DecisionKind,
    digest: String,
    options: Vec<HandoffOption>,
    recommendation: usize,
}

fn decision_point_payload_json(
    kind: DecisionKind,
    digest: &str,
    options: &[HandoffOption],
    recommendation: usize,
) -> String {
    serde_json::to_string(&DecisionPointPayload {
        kind,
        digest: digest.to_string(),
        options: options.to_vec(),
        recommendation,
    })
    .expect("decision point payload serializes to JSON")
}

fn decision_point_payload_from_json(
    event_id: &str,
    payload_json: &str,
) -> Result<DecisionPointPayload, StorageError> {
    serde_json::from_str(payload_json).map_err(|err| {
        StorageError::InvalidInput(format!(
            "decision point event {event_id} has invalid payload: {err}"
        ))
    })
}

#[derive(Debug, Serialize, Deserialize)]
struct ResolutionPayload {
    decision_point_id: String,
    chosen_index: usize,
    note: String,
}

fn resolution_payload_json(decision_point_id: &str, chosen_index: usize, note: &str) -> String {
    serde_json::to_string(&ResolutionPayload {
        decision_point_id: decision_point_id.to_string(),
        chosen_index,
        note: note.to_string(),
    })
    .expect("resolution payload serializes to JSON")
}

fn resolution_payload_from_json(
    event_id: &str,
    payload_json: &str,
) -> Result<ResolutionPayload, StorageError> {
    serde_json::from_str(payload_json).map_err(|err| {
        StorageError::InvalidInput(format!(
            "handoff resolution {event_id} has invalid payload: {err}"
        ))
    })
}

fn decision_point_from_raised_event(
    id: String,
    payload_json: &str,
    created_at: i64,
) -> Result<DecisionPoint, StorageError> {
    let payload = decision_point_payload_from_json(&id, payload_json)?;
    validate_raise_input(&payload.digest, &payload.options, payload.recommendation)?;

    Ok(DecisionPoint {
        id,
        kind: payload.kind,
        digest: payload.digest,
        options: payload.options,
        recommendation: payload.recommendation,
        status: DecisionStatus::Pending,
        resolution: None,
        created_at,
    })
}

fn apply_resolution(
    point: &mut DecisionPoint,
    event_id: &str,
    payload: ResolutionPayload,
    created_at: i64,
) -> Result<(), StorageError> {
    if point.status == DecisionStatus::Resolved {
        return Ok(());
    }

    if payload.chosen_index >= point.options.len() {
        return Err(StorageError::InvalidInput(format!(
            "handoff resolution {event_id} has invalid chosen_index {}",
            payload.chosen_index
        )));
    }
    point.status = DecisionStatus::Resolved;
    point.resolution = Some(Resolution {
        chosen_index: payload.chosen_index,
        note: payload.note,
        resolved_at: created_at,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::storage::{now_unix_seconds, EventRecord, ProjectStore, StorageError};

    use super::{
        classify, Cost, DecisionKind, DecisionStatus, DefaultPolicy, HandoffOption,
        InterventionPolicy, Risk, StepContext, TaskClass,
    };

    fn temp_project_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentflow-handoff-{test_name}-{}-{}",
            std::process::id(),
            now_unix_seconds()
        ))
    }

    fn base_context() -> StepContext {
        StepContext {
            cost: Cost::Cheap,
            reversible: true,
            equivalent_branches: false,
            conflicts_user_premise: false,
            mutates_goal: false,
            near_budget: false,
        }
    }

    fn option(label: &str) -> HandoffOption {
        HandoffOption {
            label: label.to_string(),
            direction: format!("take {label}"),
            cost: Cost::Cheap,
            risk: Risk::Low,
            reversible: true,
        }
    }

    fn raise_demo_point(store: &ProjectStore) -> String {
        store
            .raise_decision_point(
                DecisionKind::DeepenOrStop,
                "checked local evidence and found two viable paths",
                vec![option("continue"), option("stop")],
                0,
            )
            .unwrap()
            .id
    }

    #[test]
    fn policy_goal_mutation_has_highest_priority() {
        let policy = DefaultPolicy;
        let mut ctx = base_context();
        ctx.mutates_goal = true;
        ctx.conflicts_user_premise = true;
        ctx.near_budget = true;
        ctx.cost = Cost::Expensive;

        assert_eq!(policy.assess(&ctx), Some(DecisionKind::GoalMutation));
    }

    #[test]
    fn policy_conflicting_premise_precedes_budget_and_cost() {
        let policy = DefaultPolicy;
        let mut ctx = base_context();
        ctx.conflicts_user_premise = true;
        ctx.near_budget = true;
        ctx.cost = Cost::Expensive;

        assert_eq!(policy.assess(&ctx), Some(DecisionKind::PremiseChallenged));
    }

    #[test]
    fn policy_budget_threshold_precedes_cost() {
        let policy = DefaultPolicy;
        let mut ctx = base_context();
        ctx.near_budget = true;
        ctx.cost = Cost::Expensive;

        assert_eq!(policy.assess(&ctx), Some(DecisionKind::BudgetThreshold));
    }

    #[test]
    fn policy_expensive_irreversible_or_branching_intervenes() {
        let policy = DefaultPolicy;
        let mut expensive = base_context();
        expensive.cost = Cost::Expensive;
        assert_eq!(policy.assess(&expensive), Some(DecisionKind::DeepenOrStop));

        let mut irreversible = base_context();
        irreversible.reversible = false;
        assert_eq!(
            policy.assess(&irreversible),
            Some(DecisionKind::DeepenOrStop)
        );

        let mut branching = base_context();
        branching.equivalent_branches = true;
        assert_eq!(policy.assess(&branching), Some(DecisionKind::DeepenOrStop));
    }

    #[test]
    fn policy_allows_cheap_reversible_single_path() {
        let policy = DefaultPolicy;

        assert_eq!(policy.assess(&base_context()), None);
    }

    #[test]
    fn classify_respects_consequence_and_user_care() {
        assert_eq!(classify(true, true), TaskClass::Decision);
        assert_eq!(classify(true, false), TaskClass::Labor);
        assert_eq!(classify(false, true), TaskClass::Labor);
        assert_eq!(classify(false, false), TaskClass::Labor);
    }

    #[test]
    fn enum_parsers_round_trip() {
        for cost in [Cost::Cheap, Cost::Moderate, Cost::Expensive] {
            assert_eq!(Cost::parse(cost.as_str()), Some(cost));
        }
        for risk in [Risk::Low, Risk::Medium, Risk::High] {
            assert_eq!(Risk::parse(risk.as_str()), Some(risk));
        }
        for kind in [
            DecisionKind::DeepenOrStop,
            DecisionKind::PremiseChallenged,
            DecisionKind::BudgetThreshold,
            DecisionKind::GoalMutation,
            DecisionKind::ToolGap,
            DecisionKind::StanceAssessment,
        ] {
            assert_eq!(DecisionKind::parse(kind.as_str()), Some(kind));
        }
    }

    #[test]
    fn enum_json_strings_match_display_contract() {
        for cost in [Cost::Cheap, Cost::Moderate, Cost::Expensive] {
            assert_eq!(
                serde_json::to_string(&cost).unwrap(),
                format!("\"{}\"", cost.as_str())
            );
        }
        for risk in [Risk::Low, Risk::Medium, Risk::High] {
            assert_eq!(
                serde_json::to_string(&risk).unwrap(),
                format!("\"{}\"", risk.as_str())
            );
        }
        for kind in [
            DecisionKind::DeepenOrStop,
            DecisionKind::PremiseChallenged,
            DecisionKind::BudgetThreshold,
            DecisionKind::GoalMutation,
            DecisionKind::ToolGap,
            DecisionKind::StanceAssessment,
        ] {
            assert_eq!(
                serde_json::to_string(&kind).unwrap(),
                format!("\"{}\"", kind.as_str())
            );
        }
        for status in [DecisionStatus::Pending, DecisionStatus::Resolved] {
            assert_eq!(
                serde_json::to_string(&status).unwrap(),
                format!("\"{}\"", status.as_str())
            );
        }
    }

    #[test]
    fn raise_rejects_empty_digest() {
        let path = temp_project_path("empty-digest");
        let store = ProjectStore::init(&path, Some("Handoff Demo")).unwrap();
        let error = store
            .raise_decision_point(DecisionKind::DeepenOrStop, " ", vec![option("go")], 0)
            .unwrap_err();

        assert!(matches!(error, StorageError::InvalidInput(_)));
        assert!(error.to_string().contains("digest"));
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn raise_rejects_empty_options() {
        let path = temp_project_path("empty-options");
        let store = ProjectStore::init(&path, Some("Handoff Demo")).unwrap();
        let error = store
            .raise_decision_point(DecisionKind::DeepenOrStop, "did the work", Vec::new(), 0)
            .unwrap_err();

        assert!(matches!(error, StorageError::InvalidInput(_)));
        assert!(error.to_string().contains("options"));
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn raise_rejects_out_of_bounds_recommendation() {
        let path = temp_project_path("bad-recommendation");
        let store = ProjectStore::init(&path, Some("Handoff Demo")).unwrap();
        let error = store
            .raise_decision_point(
                DecisionKind::DeepenOrStop,
                "did the work",
                vec![option("go")],
                1,
            )
            .unwrap_err();

        assert!(matches!(error, StorageError::InvalidInput(_)));
        assert!(error.to_string().contains("recommendation"));
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn raises_lists_inspects_and_resolves_decision_points() {
        let path = temp_project_path("round-trip");
        let store = ProjectStore::init(&path, Some("Handoff Demo")).unwrap();
        let raised = store
            .raise_decision_point(
                DecisionKind::PremiseChallenged,
                "validated the premise against the local report",
                vec![option("challenge"), option("accept")],
                1,
            )
            .unwrap();

        assert!(raised.id.starts_with("event_"));
        assert_eq!(raised.kind, DecisionKind::PremiseChallenged);
        assert_eq!(
            raised.digest,
            "validated the premise against the local report"
        );
        assert_eq!(raised.recommendation, 1);
        assert_eq!(raised.status, DecisionStatus::Pending);
        assert_eq!(store.pending_decision_points().unwrap().len(), 1);

        let listed = store.list_decision_points().unwrap();
        assert_eq!(listed, vec![raised.clone()]);

        let inspected = store.inspect_decision_point(&raised.id).unwrap();
        assert_eq!(inspected.options[1].label, "accept");

        let resolved = store
            .resolve_decision_point(&raised.id, 1, "accept the premise")
            .unwrap();
        assert_eq!(resolved.status, DecisionStatus::Resolved);
        assert_eq!(resolved.resolution.as_ref().unwrap().chosen_index, 1);
        assert_eq!(
            resolved.resolution.as_ref().unwrap().note,
            "accept the premise"
        );
        assert!(resolved.resolution.as_ref().unwrap().resolved_at >= raised.created_at);
        assert!(store.pending_decision_points().unwrap().is_empty());
        assert_eq!(store.inspect_decision_point(&raised.id).unwrap(), resolved);

        let mut stmt = store
            .connection()
            .prepare("SELECT event_type FROM events ORDER BY created_at ASC, id ASC")
            .unwrap();
        let event_types = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            event_types,
            vec![
                "handoff.decision_point_raised".to_string(),
                "handoff.user_resolved".to_string()
            ]
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn legacy_handwritten_payloads_parse_with_json_whitespace_and_ordering() {
        let path = temp_project_path("legacy-payload");
        let store = ProjectStore::init(&path, Some("Handoff Demo")).unwrap();
        let decision_id = store
            .append_event(EventRecord {
                flow_id: None,
                step_id: None,
                run_id: None,
                event_type: super::DECISION_POINT_RAISED_EVENT.to_string(),
                payload_json: r#"{
                    "recommendation": 1,
                    "options": [
                        {
                            "risk": "medium",
                            "reversible": true,
                            "direction": "continue with \"extra\" evidence",
                            "cost": "moderate",
                            "label": "continue"
                        },
                        {
                            "label": "stop",
                            "direction": "stop and summarize",
                            "cost": "cheap",
                            "risk": "low",
                            "reversible": true
                        }
                    ],
                    "digest": "Legacy payload parses",
                    "kind": "deepen_or_stop"
                }"#
                .to_string(),
            })
            .unwrap();
        store
            .append_event(EventRecord {
                flow_id: None,
                step_id: None,
                run_id: None,
                event_type: super::USER_RESOLVED_EVENT.to_string(),
                payload_json: format!(
                    r#"{{
                        "note": "choose stop",
                        "chosen_index": 1,
                        "decision_point_id": "{decision_id}"
                    }}"#
                ),
            })
            .unwrap();

        let inspected = store.inspect_decision_point(&decision_id).unwrap();
        assert_eq!(inspected.kind, DecisionKind::DeepenOrStop);
        assert_eq!(inspected.digest, "Legacy payload parses");
        assert_eq!(inspected.options[0].label, "continue");
        assert_eq!(inspected.options[0].risk, Risk::Medium);
        assert_eq!(inspected.recommendation, 1);
        assert_eq!(inspected.status, DecisionStatus::Resolved);
        assert_eq!(inspected.resolution.as_ref().unwrap().chosen_index, 1);
        assert_eq!(inspected.resolution.as_ref().unwrap().note, "choose stop");

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn json_outputs_match_legacy_bytes() {
        let point = super::DecisionPoint {
            id: "event_1".to_string(),
            kind: DecisionKind::ToolGap,
            digest: "Quote \" and newline\nslash \\ tab\t".to_string(),
            options: vec![
                super::HandoffOption {
                    label: "continue".to_string(),
                    direction: "Use tool \"A\"\nthen B".to_string(),
                    cost: Cost::Moderate,
                    risk: Risk::Medium,
                    reversible: true,
                },
                super::HandoffOption {
                    label: "stop".to_string(),
                    direction: "Stop".to_string(),
                    cost: Cost::Cheap,
                    risk: Risk::Low,
                    reversible: false,
                },
            ],
            recommendation: 0,
            status: DecisionStatus::Resolved,
            resolution: Some(super::Resolution {
                chosen_index: 0,
                note: "accepted \"continue\"".to_string(),
                resolved_at: 22,
            }),
            created_at: 11,
        };

        assert_eq!(
            point.to_json(),
            "{\"id\":\"event_1\",\"kind\":\"tool_gap\",\"digest\":\"Quote \\\" and newline\\nslash \\\\ tab\\t\",\"options\":[{\"label\":\"continue\",\"direction\":\"Use tool \\\"A\\\"\\nthen B\",\"cost\":\"moderate\",\"risk\":\"medium\",\"reversible\":true},{\"label\":\"stop\",\"direction\":\"Stop\",\"cost\":\"cheap\",\"risk\":\"low\",\"reversible\":false}],\"recommendation\":0,\"status\":\"resolved\",\"resolution\":{\"chosen_index\":0,\"note\":\"accepted \\\"continue\\\"\",\"resolved_at\":22},\"created_at\":11}"
        );
        assert_eq!(
            super::decision_point_payload_json(
                DecisionKind::StanceAssessment,
                "Digest \"x\"",
                &point.options,
                1,
            ),
            "{\"kind\":\"stance_assessment\",\"digest\":\"Digest \\\"x\\\"\",\"options\":[{\"label\":\"continue\",\"direction\":\"Use tool \\\"A\\\"\\nthen B\",\"cost\":\"moderate\",\"risk\":\"medium\",\"reversible\":true},{\"label\":\"stop\",\"direction\":\"Stop\",\"cost\":\"cheap\",\"risk\":\"low\",\"reversible\":false}],\"recommendation\":1}"
        );
        assert_eq!(
            super::resolution_payload_json("event_1", 0, "note\nwith tab\t"),
            "{\"decision_point_id\":\"event_1\",\"chosen_index\":0,\"note\":\"note\\nwith tab\\t\"}"
        );
    }

    #[test]
    fn reverted_handoff_events_restore_pending_state_and_hide_later_points() {
        let path = temp_project_path("reverted-events");
        let store = ProjectStore::init(&path, Some("Handoff Demo")).unwrap();
        let kept_id = raise_demo_point(&store);
        let checkpoint = store.create_checkpoint("before-resolution").unwrap();
        store
            .resolve_decision_point(&kept_id, 0, "continue")
            .unwrap();
        let removed_id = store
            .raise_decision_point(
                DecisionKind::BudgetThreshold,
                "later budget threshold",
                vec![option("pause"), option("continue")],
                0,
            )
            .unwrap()
            .id;

        assert_eq!(
            store.inspect_decision_point(&kept_id).unwrap().status,
            DecisionStatus::Resolved
        );
        assert_eq!(store.list_decision_points().unwrap().len(), 2);

        store.revert_to(&checkpoint.id).unwrap();

        let points = store.list_decision_points().unwrap();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].id, kept_id);
        assert_eq!(points[0].status, DecisionStatus::Pending);
        assert!(points[0].resolution.is_none());
        assert_eq!(store.pending_decision_points().unwrap().len(), 1);
        assert!(matches!(
            store.inspect_decision_point(&removed_id).unwrap_err(),
            StorageError::NotFound(_)
        ));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn resolve_rejects_duplicate_resolution() {
        let path = temp_project_path("duplicate-resolution");
        let store = ProjectStore::init(&path, Some("Handoff Demo")).unwrap();
        let id = raise_demo_point(&store);
        store.resolve_decision_point(&id, 0, "go").unwrap();

        let error = store.resolve_decision_point(&id, 1, "stop").unwrap_err();
        assert!(matches!(error, StorageError::InvalidInput(_)));
        assert!(error.to_string().contains("already been resolved"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn resolve_rejects_invalid_chosen_index() {
        let path = temp_project_path("bad-resolution-index");
        let store = ProjectStore::init(&path, Some("Handoff Demo")).unwrap();
        let id = raise_demo_point(&store);

        let error = store.resolve_decision_point(&id, 2, "bad").unwrap_err();
        assert!(matches!(error, StorageError::InvalidInput(_)));
        assert!(error.to_string().contains("chosen_index"));
        assert_eq!(
            store.inspect_decision_point(&id).unwrap().status,
            DecisionStatus::Pending
        );

        let _ = std::fs::remove_dir_all(path);
    }
}
