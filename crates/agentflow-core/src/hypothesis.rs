use std::fmt;

use rusqlite::params;

use crate::storage::{EventRecord, ProjectStore, StorageError};

const HYPOTHESIS_CREATED_EVENT: &str = "hypothesis.created";
const HYPOTHESIS_TRANSITIONED_EVENT: &str = "hypothesis.transitioned";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl Confidence {
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

impl fmt::Display for Confidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HypothesisStatus {
    Proposed,
    UnderTest,
    Supported,
    Weakened,
    Contradicted,
    Inconclusive,
    Superseded,
}

impl HypothesisStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::UnderTest => "under_test",
            Self::Supported => "supported",
            Self::Weakened => "weakened",
            Self::Contradicted => "contradicted",
            Self::Inconclusive => "inconclusive",
            Self::Superseded => "superseded",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "proposed" => Some(Self::Proposed),
            "under_test" => Some(Self::UnderTest),
            "supported" => Some(Self::Supported),
            "weakened" => Some(Self::Weakened),
            "contradicted" => Some(Self::Contradicted),
            "inconclusive" => Some(Self::Inconclusive),
            "superseded" => Some(Self::Superseded),
            _ => None,
        }
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Proposed, Self::UnderTest)
                | (Self::Proposed, Self::Superseded)
                | (Self::UnderTest, Self::Supported)
                | (Self::UnderTest, Self::Weakened)
                | (Self::UnderTest, Self::Contradicted)
                | (Self::UnderTest, Self::Inconclusive)
                | (Self::UnderTest, Self::Superseded)
                | (Self::Weakened, Self::UnderTest)
                | (Self::Weakened, Self::Contradicted)
                | (Self::Weakened, Self::Inconclusive)
                | (Self::Weakened, Self::Superseded)
                | (Self::Supported, Self::Weakened)
                | (Self::Supported, Self::Superseded)
                | (Self::Contradicted, Self::Superseded)
                | (Self::Inconclusive, Self::UnderTest)
                | (Self::Inconclusive, Self::Superseded)
        )
    }
}

impl fmt::Display for HypothesisStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hypothesis {
    pub id: String,
    pub statement: String,
    pub origin: String,
    pub related_goal_id: String,
    pub status: HypothesisStatus,
    pub confidence: Confidence,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HypothesisRequest {
    pub statement: String,
    pub origin: String,
    pub related_goal_id: String,
}

impl ProjectStore {
    pub fn record_hypothesis(
        &self,
        request: HypothesisRequest,
    ) -> Result<Hypothesis, StorageError> {
        validate_hypothesis_request(&request)?;
        let payload_json = hypothesis_created_payload_json(&request);
        let id = self.append_event(EventRecord {
            flow_id: None,
            step_id: None,
            run_id: None,
            event_type: HYPOTHESIS_CREATED_EVENT.to_string(),
            payload_json,
        })?;
        self.touch_project()?;
        self.inspect_hypothesis(&id)
    }

    pub fn list_hypotheses(&self) -> Result<Vec<Hypothesis>, StorageError> {
        let mut stmt = self.connection().prepare(
            "SELECT id, event_type, payload_json, created_at
             FROM events
             WHERE event_type IN (?1, ?2)
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map(
            params![HYPOTHESIS_CREATED_EVENT, HYPOTHESIS_TRANSITIONED_EVENT],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )?;

        let mut hypotheses = Vec::new();
        for row in rows {
            let (event_id, event_type, payload_json, created_at) = row?;
            match event_type.as_str() {
                HYPOTHESIS_CREATED_EVENT => {
                    hypotheses.push(hypothesis_from_created_event(
                        event_id,
                        &payload_json,
                        created_at,
                    )?);
                }
                HYPOTHESIS_TRANSITIONED_EVENT => {
                    let hypothesis_id =
                        required_json_string(&event_id, &payload_json, "hypothesis_id")?;
                    if let Some(hypothesis) = hypotheses
                        .iter_mut()
                        .find(|hypothesis: &&mut Hypothesis| hypothesis.id == hypothesis_id)
                    {
                        hypothesis.status = parse_status(&event_id, &payload_json, "status")?;
                        hypothesis.confidence =
                            parse_confidence(&event_id, &payload_json, "confidence")?;
                        hypothesis.updated_at = created_at;
                    }
                }
                _ => {}
            }
        }
        Ok(hypotheses)
    }

    pub fn inspect_hypothesis(&self, id: &str) -> Result<Hypothesis, StorageError> {
        if id.trim().is_empty() {
            return Err(StorageError::InvalidInput(
                "hypothesis id must not be empty".to_string(),
            ));
        }

        self.list_hypotheses()?
            .into_iter()
            .find(|hypothesis| hypothesis.id == id)
            .ok_or_else(|| StorageError::NotFound(format!("hypothesis {id}")))
    }

    pub fn transition_hypothesis(
        &self,
        id: &str,
        next: HypothesisStatus,
        confidence: Confidence,
    ) -> Result<Hypothesis, StorageError> {
        let current = self.inspect_hypothesis(id)?;
        if !current.status.can_transition_to(next) {
            return Err(StorageError::InvalidInput(format!(
                "hypothesis {id} cannot transition from {} to {}",
                current.status, next
            )));
        }

        self.append_event(EventRecord {
            flow_id: None,
            step_id: None,
            run_id: None,
            event_type: HYPOTHESIS_TRANSITIONED_EVENT.to_string(),
            payload_json: hypothesis_transitioned_payload_json(id, next, confidence),
        })?;
        self.touch_project()?;
        self.inspect_hypothesis(id)
    }
}

fn validate_hypothesis_request(request: &HypothesisRequest) -> Result<(), StorageError> {
    validate_non_empty("statement", &request.statement)?;
    validate_non_empty("origin", &request.origin)?;
    validate_non_empty("related_goal_id", &request.related_goal_id)?;
    Ok(())
}

fn validate_non_empty(label: &str, value: &str) -> Result<(), StorageError> {
    if value.trim().is_empty() {
        Err(StorageError::InvalidInput(format!(
            "hypothesis {label} must not be empty"
        )))
    } else {
        Ok(())
    }
}

fn hypothesis_created_payload_json(request: &HypothesisRequest) -> String {
    format!(
        concat!(
            "{{",
            "\"statement\":\"{}\",",
            "\"origin\":\"{}\",",
            "\"related_goal_id\":\"{}\",",
            "\"status\":\"{}\",",
            "\"confidence\":\"{}\"",
            "}}"
        ),
        escape_json(request.statement.trim()),
        escape_json(request.origin.trim()),
        escape_json(request.related_goal_id.trim()),
        HypothesisStatus::Proposed.as_str(),
        Confidence::Low.as_str()
    )
}

fn hypothesis_transitioned_payload_json(
    hypothesis_id: &str,
    status: HypothesisStatus,
    confidence: Confidence,
) -> String {
    format!(
        concat!(
            "{{",
            "\"hypothesis_id\":\"{}\",",
            "\"status\":\"{}\",",
            "\"confidence\":\"{}\"",
            "}}"
        ),
        escape_json(hypothesis_id.trim()),
        status.as_str(),
        confidence.as_str()
    )
}

fn hypothesis_from_created_event(
    id: String,
    payload_json: &str,
    created_at: i64,
) -> Result<Hypothesis, StorageError> {
    Ok(Hypothesis {
        id: id.clone(),
        statement: required_json_string(&id, payload_json, "statement")?,
        origin: required_json_string(&id, payload_json, "origin")?,
        related_goal_id: required_json_string(&id, payload_json, "related_goal_id")?,
        status: parse_status(&id, payload_json, "status")?,
        confidence: parse_confidence(&id, payload_json, "confidence")?,
        created_at,
        updated_at: created_at,
    })
}

fn required_json_string(
    event_id: &str,
    payload_json: &str,
    field: &str,
) -> Result<String, StorageError> {
    json_string_field(payload_json, field).ok_or_else(|| {
        StorageError::InvalidInput(format!("hypothesis event {event_id} is missing {field}"))
    })
}

fn parse_status(
    event_id: &str,
    payload_json: &str,
    field: &str,
) -> Result<HypothesisStatus, StorageError> {
    let value = required_json_string(event_id, payload_json, field)?;
    HypothesisStatus::parse(&value).ok_or_else(|| {
        StorageError::InvalidInput(format!(
            "hypothesis event {event_id} has invalid status {value}"
        ))
    })
}

fn parse_confidence(
    event_id: &str,
    payload_json: &str,
    field: &str,
) -> Result<Confidence, StorageError> {
    let value = required_json_string(event_id, payload_json, field)?;
    Confidence::parse(&value).ok_or_else(|| {
        StorageError::InvalidInput(format!(
            "hypothesis event {event_id} has invalid confidence {value}"
        ))
    })
}

fn json_string_field(json: &str, field: &str) -> Option<String> {
    let marker = format!("\"{field}\":\"");
    let start = json.find(&marker)? + marker.len();
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

    use crate::storage::{now_unix_seconds, ProjectStore};

    use super::{Confidence, HypothesisRequest, HypothesisStatus};

    fn temp_project_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentflow-hypothesis-{test_name}-{}-{}",
            std::process::id(),
            now_unix_seconds()
        ))
    }

    #[test]
    fn records_lists_and_inspects_hypotheses() {
        let path = temp_project_path("record");
        let store = ProjectStore::init(&path, Some("Hypothesis Demo")).unwrap();
        let hypothesis = store
            .record_hypothesis(HypothesisRequest {
                statement: " Marker A validates pathway B ".to_string(),
                origin: " user_goal ".to_string(),
                related_goal_id: " goal_1 ".to_string(),
            })
            .unwrap();

        assert!(hypothesis.id.starts_with("event_"));
        assert_eq!(hypothesis.statement, "Marker A validates pathway B");
        assert_eq!(hypothesis.origin, "user_goal");
        assert_eq!(hypothesis.related_goal_id, "goal_1");
        assert_eq!(hypothesis.status, HypothesisStatus::Proposed);
        assert_eq!(hypothesis.confidence, Confidence::Low);
        assert_eq!(hypothesis.created_at, hypothesis.updated_at);

        let hypotheses = store.list_hypotheses().unwrap();
        assert_eq!(hypotheses.len(), 1);
        assert_eq!(hypotheses[0].id, hypothesis.id);

        let inspected = store.inspect_hypothesis(&hypothesis.id).unwrap();
        assert_eq!(inspected.statement, "Marker A validates pathway B");

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn transitions_hypotheses_by_replaying_events() {
        let path = temp_project_path("transition");
        let store = ProjectStore::init(&path, Some("Hypothesis Demo")).unwrap();
        let hypothesis = store
            .record_hypothesis(HypothesisRequest {
                statement: "Candidate pathway is enriched".to_string(),
                origin: "agent".to_string(),
                related_goal_id: "goal_2".to_string(),
            })
            .unwrap();

        let under_test = store
            .transition_hypothesis(
                &hypothesis.id,
                HypothesisStatus::UnderTest,
                Confidence::Medium,
            )
            .unwrap();
        assert_eq!(under_test.status, HypothesisStatus::UnderTest);
        assert_eq!(under_test.confidence, Confidence::Medium);

        let supported = store
            .transition_hypothesis(
                &hypothesis.id,
                HypothesisStatus::Supported,
                Confidence::High,
            )
            .unwrap();
        assert_eq!(supported.status, HypothesisStatus::Supported);
        assert_eq!(supported.confidence, Confidence::High);
        assert!(supported.updated_at >= supported.created_at);

        let hypotheses = store.list_hypotheses().unwrap();
        assert_eq!(hypotheses[0].status, HypothesisStatus::Supported);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn rejects_invalid_hypothesis_requests() {
        let path = temp_project_path("invalid-request");
        let store = ProjectStore::init(&path, Some("Hypothesis Demo")).unwrap();
        let error = store
            .record_hypothesis(HypothesisRequest {
                statement: " ".to_string(),
                origin: "agent".to_string(),
                related_goal_id: "goal_3".to_string(),
            })
            .unwrap_err();

        assert!(error.to_string().contains("hypothesis statement"));
        assert!(store.list_hypotheses().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn rejects_illegal_hypothesis_transitions() {
        let path = temp_project_path("illegal-transition");
        let store = ProjectStore::init(&path, Some("Hypothesis Demo")).unwrap();
        let hypothesis = store
            .record_hypothesis(HypothesisRequest {
                statement: "The result is reproducible".to_string(),
                origin: "agent".to_string(),
                related_goal_id: "goal_4".to_string(),
            })
            .unwrap();

        let error = store
            .transition_hypothesis(
                &hypothesis.id,
                HypothesisStatus::Supported,
                Confidence::Medium,
            )
            .unwrap_err();
        assert!(error.to_string().contains("cannot transition"));
        assert_eq!(
            store.inspect_hypothesis(&hypothesis.id).unwrap().status,
            HypothesisStatus::Proposed
        );

        store
            .transition_hypothesis(
                &hypothesis.id,
                HypothesisStatus::Superseded,
                Confidence::Low,
            )
            .unwrap();
        let error = store
            .transition_hypothesis(&hypothesis.id, HypothesisStatus::UnderTest, Confidence::Low)
            .unwrap_err();
        assert!(error.to_string().contains("cannot transition"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn hypothesis_status_transition_rules_match_contract() {
        assert!(HypothesisStatus::Proposed.can_transition_to(HypothesisStatus::UnderTest));
        assert!(HypothesisStatus::UnderTest.can_transition_to(HypothesisStatus::Supported));
        assert!(HypothesisStatus::UnderTest.can_transition_to(HypothesisStatus::Contradicted));
        assert!(HypothesisStatus::Weakened.can_transition_to(HypothesisStatus::UnderTest));
        assert!(HypothesisStatus::Supported.can_transition_to(HypothesisStatus::Weakened));
        assert!(HypothesisStatus::Contradicted.can_transition_to(HypothesisStatus::Superseded));
        assert!(HypothesisStatus::Inconclusive.can_transition_to(HypothesisStatus::UnderTest));

        assert!(!HypothesisStatus::Proposed.can_transition_to(HypothesisStatus::Proposed));
        assert!(!HypothesisStatus::Proposed.can_transition_to(HypothesisStatus::Supported));
        assert!(!HypothesisStatus::Superseded.can_transition_to(HypothesisStatus::UnderTest));
    }
}
