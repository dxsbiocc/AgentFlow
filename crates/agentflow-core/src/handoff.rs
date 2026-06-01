use std::fmt;

use rusqlite::params;

use crate::storage::{EventRecord, ProjectStore, StorageError};

const DECISION_POINT_RAISED_EVENT: &str = "handoff.decision_point_raised";
const USER_RESOLVED_EVENT: &str = "handoff.user_resolved";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecisionKind {
    DeepenOrStop,
    PremiseChallenged,
    BudgetThreshold,
    GoalMutation,
}

impl DecisionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DeepenOrStop => "deepen_or_stop",
            Self::PremiseChallenged => "premise_challenged",
            Self::BudgetThreshold => "budget_threshold",
            Self::GoalMutation => "goal_mutation",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "deepen_or_stop" => Some(Self::DeepenOrStop),
            "premise_challenged" => Some(Self::PremiseChallenged),
            "budget_threshold" => Some(Self::BudgetThreshold),
            "goal_mutation" => Some(Self::GoalMutation),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffOption {
    pub label: String,
    pub direction: String,
    pub cost: Cost,
    pub risk: Risk,
    pub reversible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecisionStatus {
    Pending,
    Resolved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub chosen_index: usize,
    pub note: String,
    pub resolved_at: i64,
}

impl DecisionPoint {
    pub fn to_json(&self) -> String {
        format!(
            concat!(
                "{{",
                "\"id\":\"{}\",",
                "\"kind\":\"{}\",",
                "\"digest\":\"{}\",",
                "\"options\":{},",
                "\"recommendation\":{},",
                "\"status\":\"{}\",",
                "\"resolution\":{},",
                "\"created_at\":{}",
                "}}"
            ),
            escape_json(&self.id),
            self.kind.as_str(),
            escape_json(&self.digest),
            handoff_options_json(&self.options),
            self.recommendation,
            decision_status_as_str(self.status),
            resolution_json(self.resolution.as_ref()),
            self.created_at
        )
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
                    let decision_point_id =
                        required_json_string(&event_id, &payload_json, "decision_point_id")?;
                    if let Some(point) = points
                        .iter_mut()
                        .find(|point: &&mut DecisionPoint| point.id == decision_point_id)
                    {
                        apply_resolution(point, &event_id, &payload_json, created_at)?;
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

fn decision_point_payload_json(
    kind: DecisionKind,
    digest: &str,
    options: &[HandoffOption],
    recommendation: usize,
) -> String {
    let options_json = options
        .iter()
        .map(handoff_option_payload_json)
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"kind\":\"{}\",\"digest\":\"{}\",\"options\":[{}],\"recommendation\":{}}}",
        kind.as_str(),
        escape_json(digest),
        options_json,
        recommendation
    )
}

fn handoff_option_payload_json(option: &HandoffOption) -> String {
    format!(
        concat!(
            "{{",
            "\"label\":\"{}\",",
            "\"direction\":\"{}\",",
            "\"cost\":\"{}\",",
            "\"risk\":\"{}\",",
            "\"reversible\":{}",
            "}}"
        ),
        escape_json(&option.label),
        escape_json(&option.direction),
        option.cost.as_str(),
        option.risk.as_str(),
        option.reversible
    )
}

fn resolution_payload_json(decision_point_id: &str, chosen_index: usize, note: &str) -> String {
    format!(
        "{{\"decision_point_id\":\"{}\",\"chosen_index\":{},\"note\":\"{}\"}}",
        escape_json(decision_point_id),
        chosen_index,
        escape_json(note)
    )
}

fn decision_status_as_str(status: DecisionStatus) -> &'static str {
    match status {
        DecisionStatus::Pending => "pending",
        DecisionStatus::Resolved => "resolved",
    }
}

fn handoff_options_json(options: &[HandoffOption]) -> String {
    let items = options
        .iter()
        .map(handoff_option_json)
        .collect::<Vec<_>>()
        .join(",");
    format!("[{items}]")
}

fn handoff_option_json(option: &HandoffOption) -> String {
    format!(
        concat!(
            "{{",
            "\"label\":\"{}\",",
            "\"direction\":\"{}\",",
            "\"cost\":\"{}\",",
            "\"risk\":\"{}\",",
            "\"reversible\":{}",
            "}}"
        ),
        escape_json(&option.label),
        escape_json(&option.direction),
        option.cost.as_str(),
        option.risk.as_str(),
        option.reversible
    )
}

fn resolution_json(resolution: Option<&Resolution>) -> String {
    resolution.map_or_else(
        || "null".to_string(),
        |resolution| {
            format!(
                concat!(
                    "{{",
                    "\"chosen_index\":{},",
                    "\"note\":\"{}\",",
                    "\"resolved_at\":{}",
                    "}}"
                ),
                resolution.chosen_index,
                escape_json(&resolution.note),
                resolution.resolved_at
            )
        },
    )
}

fn decision_point_from_raised_event(
    id: String,
    payload_json: &str,
    created_at: i64,
) -> Result<DecisionPoint, StorageError> {
    let kind = parse_decision_kind(&id, payload_json, "kind")?;
    let digest = required_json_string(&id, payload_json, "digest")?;
    let options = parse_options(&id, payload_json)?;
    let recommendation = required_json_usize(&id, payload_json, "recommendation")?;
    validate_raise_input(&digest, &options, recommendation)?;

    Ok(DecisionPoint {
        id,
        kind,
        digest,
        options,
        recommendation,
        status: DecisionStatus::Pending,
        resolution: None,
        created_at,
    })
}

fn apply_resolution(
    point: &mut DecisionPoint,
    event_id: &str,
    payload_json: &str,
    created_at: i64,
) -> Result<(), StorageError> {
    if point.status == DecisionStatus::Resolved {
        return Ok(());
    }

    let chosen_index = required_json_usize(event_id, payload_json, "chosen_index")?;
    if chosen_index >= point.options.len() {
        return Err(StorageError::InvalidInput(format!(
            "handoff resolution {event_id} has invalid chosen_index {chosen_index}"
        )));
    }
    point.status = DecisionStatus::Resolved;
    point.resolution = Some(Resolution {
        chosen_index,
        note: required_json_string(event_id, payload_json, "note")?,
        resolved_at: created_at,
    });
    Ok(())
}

fn parse_options(event_id: &str, payload_json: &str) -> Result<Vec<HandoffOption>, StorageError> {
    let objects = json_object_array_field(payload_json, "options").ok_or_else(|| {
        StorageError::InvalidInput(format!(
            "decision point event {event_id} is missing options"
        ))
    })?;
    objects
        .into_iter()
        .map(|object| {
            Ok(HandoffOption {
                label: required_json_string(event_id, &object, "label")?,
                direction: required_json_string(event_id, &object, "direction")?,
                cost: parse_cost(event_id, &object, "cost")?,
                risk: parse_risk(event_id, &object, "risk")?,
                reversible: required_json_bool(event_id, &object, "reversible")?,
            })
        })
        .collect()
}

fn parse_decision_kind(
    event_id: &str,
    payload_json: &str,
    field: &str,
) -> Result<DecisionKind, StorageError> {
    let value = required_json_string(event_id, payload_json, field)?;
    DecisionKind::parse(&value).ok_or_else(|| {
        StorageError::InvalidInput(format!(
            "decision point event {event_id} has invalid kind {value}"
        ))
    })
}

fn parse_cost(event_id: &str, payload_json: &str, field: &str) -> Result<Cost, StorageError> {
    let value = required_json_string(event_id, payload_json, field)?;
    Cost::parse(&value).ok_or_else(|| {
        StorageError::InvalidInput(format!(
            "decision point event {event_id} has invalid cost {value}"
        ))
    })
}

fn parse_risk(event_id: &str, payload_json: &str, field: &str) -> Result<Risk, StorageError> {
    let value = required_json_string(event_id, payload_json, field)?;
    Risk::parse(&value).ok_or_else(|| {
        StorageError::InvalidInput(format!(
            "decision point event {event_id} has invalid risk {value}"
        ))
    })
}

fn required_json_string(
    event_id: &str,
    payload_json: &str,
    field: &str,
) -> Result<String, StorageError> {
    json_string_field(payload_json, field).ok_or_else(|| {
        StorageError::InvalidInput(format!("handoff event {event_id} is missing {field}"))
    })
}

fn required_json_usize(
    event_id: &str,
    payload_json: &str,
    field: &str,
) -> Result<usize, StorageError> {
    json_usize_field(payload_json, field).ok_or_else(|| {
        StorageError::InvalidInput(format!("handoff event {event_id} is missing {field}"))
    })
}

fn required_json_bool(
    event_id: &str,
    payload_json: &str,
    field: &str,
) -> Result<bool, StorageError> {
    json_bool_field(payload_json, field).ok_or_else(|| {
        StorageError::InvalidInput(format!("handoff event {event_id} is missing {field}"))
    })
}

fn json_string_field(json: &str, field: &str) -> Option<String> {
    let marker = format!("\"{field}\":\"");
    let start = json.find(&marker)? + marker.len();
    let rest = &json[start..];
    let end = find_json_string_end(rest)?;
    Some(unescape_json_string(&rest[..end]))
}

fn json_usize_field(json: &str, field: &str) -> Option<usize> {
    let marker = format!("\"{field}\":");
    let start = json.find(&marker)? + marker.len();
    let rest = &json[start..];
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    rest[..end].trim().parse().ok()
}

fn json_bool_field(json: &str, field: &str) -> Option<bool> {
    let marker = format!("\"{field}\":");
    let start = json.find(&marker)? + marker.len();
    let rest = json[start..].trim_start();
    if rest.starts_with("true") {
        Some(true)
    } else if rest.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn json_object_array_field(json: &str, field: &str) -> Option<Vec<String>> {
    let marker = format!("\"{field}\":[");
    let mut index = json.find(&marker)? + marker.len();
    let mut objects = Vec::new();
    loop {
        index = skip_whitespace(json, index);
        let next = json[index..].chars().next()?;
        if next == ']' {
            return Some(objects);
        }
        if next != '{' {
            return None;
        }
        let end = find_matching_delimiter(json, index, '{', '}')?;
        objects.push(json[index..end + 1].to_string());
        index = skip_whitespace(json, end + 1);
        match json[index..].chars().next()? {
            ',' => index += 1,
            ']' => return Some(objects),
            _ => return None,
        }
    }
}

fn skip_whitespace(input: &str, mut index: usize) -> usize {
    while let Some(ch) = input[index..].chars().next() {
        if ch.is_whitespace() {
            index += ch.len_utf8();
        } else {
            break;
        }
    }
    index
}

fn find_matching_delimiter(input: &str, start: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0_usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in input[start..].char_indices() {
        let index = start + offset;
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
        } else if ch == open {
            depth += 1;
        } else if ch == close {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
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
        if ch == '\\' {
            match chars.next() {
                Some('"') => output.push('"'),
                Some('\\') => output.push('\\'),
                Some('n') => output.push('\n'),
                Some('r') => output.push('\r'),
                Some('t') => output.push('\t'),
                Some(other) => output.push(other),
                None => {}
            }
        } else {
            output.push(ch);
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::storage::{now_unix_seconds, ProjectStore, StorageError};

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
        ] {
            assert_eq!(DecisionKind::parse(kind.as_str()), Some(kind));
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
