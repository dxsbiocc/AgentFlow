use std::fmt;

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::storage::{EventRecord, ProjectStore, StorageError};

const HYPOTHESIS_CREATED_EVENT: &str = "hypothesis.created";
const HYPOTHESIS_TRANSITIONED_EVENT: &str = "hypothesis.transitioned";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

impl Hypothesis {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("hypothesis serializes to JSON")
    }
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
        let reverted = self.reverted_event_id_set()?;
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
            if reverted.contains(&event_id) {
                continue;
            }
            match event_type.as_str() {
                HYPOTHESIS_CREATED_EVENT => {
                    hypotheses.push(hypothesis_from_created_event(
                        event_id,
                        &payload_json,
                        created_at,
                    )?);
                }
                HYPOTHESIS_TRANSITIONED_EVENT => {
                    let payload = transitioned_payload_from_json(&event_id, &payload_json)?;
                    if let Some(hypothesis) = hypotheses
                        .iter_mut()
                        .find(|hypothesis: &&mut Hypothesis| hypothesis.id == payload.hypothesis_id)
                    {
                        hypothesis.status = payload.status;
                        hypothesis.confidence = payload.confidence;
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

#[derive(Debug, Serialize, Deserialize)]
struct HypothesisCreatedPayload {
    statement: String,
    origin: String,
    related_goal_id: String,
    status: HypothesisStatus,
    confidence: Confidence,
}

fn hypothesis_created_payload_json(request: &HypothesisRequest) -> String {
    serde_json::to_string(&HypothesisCreatedPayload {
        statement: request.statement.trim().to_string(),
        origin: request.origin.trim().to_string(),
        related_goal_id: request.related_goal_id.trim().to_string(),
        status: HypothesisStatus::Proposed,
        confidence: Confidence::Low,
    })
    .expect("hypothesis created payload serializes to JSON")
}

#[derive(Debug, Serialize, Deserialize)]
struct HypothesisTransitionedPayload {
    hypothesis_id: String,
    status: HypothesisStatus,
    confidence: Confidence,
}

fn hypothesis_transitioned_payload_json(
    hypothesis_id: &str,
    status: HypothesisStatus,
    confidence: Confidence,
) -> String {
    serde_json::to_string(&HypothesisTransitionedPayload {
        hypothesis_id: hypothesis_id.trim().to_string(),
        status,
        confidence,
    })
    .expect("hypothesis transitioned payload serializes to JSON")
}

fn hypothesis_from_created_event(
    id: String,
    payload_json: &str,
    created_at: i64,
) -> Result<Hypothesis, StorageError> {
    let payload = created_payload_from_json(&id, payload_json)?;
    Ok(Hypothesis {
        id,
        statement: payload.statement,
        origin: payload.origin,
        related_goal_id: payload.related_goal_id,
        status: payload.status,
        confidence: payload.confidence,
        created_at,
        updated_at: created_at,
    })
}

fn created_payload_from_json(
    event_id: &str,
    payload_json: &str,
) -> Result<HypothesisCreatedPayload, StorageError> {
    serde_json::from_str(payload_json).map_err(|err| {
        StorageError::InvalidInput(format!(
            "hypothesis event {event_id} has invalid payload: {err}"
        ))
    })
}

fn transitioned_payload_from_json(
    event_id: &str,
    payload_json: &str,
) -> Result<HypothesisTransitionedPayload, StorageError> {
    serde_json::from_str(payload_json).map_err(|err| {
        StorageError::InvalidInput(format!(
            "hypothesis event {event_id} has invalid payload: {err}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::storage::{now_unix_seconds, EventRecord, ProjectStore};

    use super::{Confidence, Hypothesis, HypothesisRequest, HypothesisStatus};

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
    fn reverted_transition_event_restores_previous_projection_state() {
        let path = temp_project_path("reverted-transition");
        let store = ProjectStore::init(&path, Some("Hypothesis Demo")).unwrap();
        let hypothesis = store
            .record_hypothesis(HypothesisRequest {
                statement: "Candidate pathway is enriched".to_string(),
                origin: "agent".to_string(),
                related_goal_id: "goal_revert".to_string(),
            })
            .unwrap();
        store
            .transition_hypothesis(
                &hypothesis.id,
                HypothesisStatus::UnderTest,
                Confidence::Medium,
            )
            .unwrap();
        let checkpoint = store.create_checkpoint("before-supported").unwrap();
        store
            .transition_hypothesis(
                &hypothesis.id,
                HypothesisStatus::Supported,
                Confidence::High,
            )
            .unwrap();

        assert_eq!(
            store.inspect_hypothesis(&hypothesis.id).unwrap().status,
            HypothesisStatus::Supported
        );

        store.revert_to(&checkpoint.id).unwrap();

        let inspected = store.inspect_hypothesis(&hypothesis.id).unwrap();
        assert_eq!(inspected.status, HypothesisStatus::UnderTest);
        assert_eq!(inspected.confidence, Confidence::Medium);
        assert_eq!(store.list_hypotheses().unwrap().len(), 1);

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
    fn legacy_handwritten_payloads_parse_with_json_whitespace_and_ordering() {
        let path = temp_project_path("legacy-payload");
        let store = ProjectStore::init(&path, Some("Hypothesis Demo")).unwrap();
        let hypothesis_id = store
            .append_event(EventRecord {
                flow_id: None,
                step_id: None,
                run_id: None,
                event_type: super::HYPOTHESIS_CREATED_EVENT.to_string(),
                payload_json: r#"{
                    "confidence": "low",
                    "related_goal_id": "goal_legacy",
                    "origin": "agent",
                    "status": "proposed",
                    "statement": "Legacy payload parses"
                }"#
                .to_string(),
            })
            .unwrap();

        store
            .append_event(EventRecord {
                flow_id: None,
                step_id: None,
                run_id: None,
                event_type: super::HYPOTHESIS_TRANSITIONED_EVENT.to_string(),
                payload_json: format!(
                    r#"{{
                        "confidence": "medium",
                        "status": "under_test",
                        "hypothesis_id": "{hypothesis_id}"
                    }}"#
                ),
            })
            .unwrap();

        let inspected = store.inspect_hypothesis(&hypothesis_id).unwrap();
        assert_eq!(inspected.statement, "Legacy payload parses");
        assert_eq!(inspected.origin, "agent");
        assert_eq!(inspected.related_goal_id, "goal_legacy");
        assert_eq!(inspected.status, HypothesisStatus::UnderTest);
        assert_eq!(inspected.confidence, Confidence::Medium);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn json_outputs_match_legacy_bytes() {
        let hypothesis = Hypothesis {
            id: "event_1".to_string(),
            statement: "Quote \" and newline\n".to_string(),
            origin: "agent\\cli".to_string(),
            related_goal_id: "goal_1".to_string(),
            status: HypothesisStatus::UnderTest,
            confidence: Confidence::Medium,
            created_at: 11,
            updated_at: 22,
        };

        assert_eq!(
            hypothesis.to_json(),
            "{\"id\":\"event_1\",\"statement\":\"Quote \\\" and newline\\n\",\"origin\":\"agent\\\\cli\",\"related_goal_id\":\"goal_1\",\"status\":\"under_test\",\"confidence\":\"medium\",\"created_at\":11,\"updated_at\":22}"
        );
        assert_eq!(
            super::hypothesis_created_payload_json(&HypothesisRequest {
                statement: " Statement ".to_string(),
                origin: " agent ".to_string(),
                related_goal_id: " goal_1 ".to_string(),
            }),
            "{\"statement\":\"Statement\",\"origin\":\"agent\",\"related_goal_id\":\"goal_1\",\"status\":\"proposed\",\"confidence\":\"low\"}"
        );
        assert_eq!(
            super::hypothesis_transitioned_payload_json(
                " event_1 ",
                HypothesisStatus::Supported,
                Confidence::High
            ),
            "{\"hypothesis_id\":\"event_1\",\"status\":\"supported\",\"confidence\":\"high\"}"
        );
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

    #[test]
    fn enum_json_strings_match_display_contract() {
        assert_eq!(
            serde_json::to_string(&HypothesisStatus::UnderTest).unwrap(),
            format!("\"{}\"", HypothesisStatus::UnderTest.as_str())
        );
        assert_eq!(
            serde_json::to_string(&Confidence::Low).unwrap(),
            format!("\"{}\"", Confidence::Low.as_str())
        );
    }
}
