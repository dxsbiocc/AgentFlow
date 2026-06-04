use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::storage::{EventRecord, ProjectStore, StorageError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResearchNoteRequest {
    pub problem: String,
    pub question: String,
    pub finding: String,
    pub confidence: String,
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResearchNote {
    pub id: String,
    pub problem: String,
    pub question: String,
    pub finding: String,
    pub confidence: String,
    pub source: Option<String>,
    pub created_at: i64,
}

impl ResearchNote {
    pub fn to_json(&self) -> String {
        serde_json::to_string(&ResearchNoteJson {
            id: self.id.clone(),
            problem: self.problem.clone(),
            question: self.question.clone(),
            finding: self.finding.clone(),
            confidence: self.confidence.clone(),
            source: self
                .source
                .as_ref()
                .filter(|source| !source.trim().is_empty())
                .cloned(),
            created_at: self.created_at,
        })
        .expect("research note serializes to JSON")
    }
}

impl ProjectStore {
    pub fn record_research_note(
        &self,
        request: ResearchNoteRequest,
    ) -> Result<ResearchNote, StorageError> {
        validate_research_note_request(&request)?;
        let payload_json = research_note_payload_json(&request);
        let id = self.append_event(EventRecord {
            flow_id: None,
            step_id: None,
            run_id: None,
            event_type: "research_note_recorded".to_string(),
            payload_json,
        })?;
        self.touch_project()?;
        self.inspect_research_note(&id)
    }

    pub fn list_research_notes(&self) -> Result<Vec<ResearchNote>, StorageError> {
        let mut stmt = self.connection().prepare(
            "SELECT id, payload_json, created_at
             FROM events
             WHERE event_type = 'research_note_recorded'
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let id = row.get::<_, String>(0)?;
            let payload_json = row.get::<_, String>(1)?;
            let created_at = row.get::<_, i64>(2)?;
            note_from_event(id, &payload_json, created_at)
        })?;

        let mut notes = Vec::new();
        for row in rows {
            notes.push(row?);
        }
        Ok(notes)
    }

    pub fn inspect_research_note(&self, note_id: &str) -> Result<ResearchNote, StorageError> {
        if note_id.trim().is_empty() {
            return Err(StorageError::InvalidInput(
                "research note id must not be empty".to_string(),
            ));
        }
        self.connection()
            .query_row(
                "SELECT id, payload_json, created_at
                 FROM events
                 WHERE id = ?1 AND event_type = 'research_note_recorded'",
                params![note_id],
                |row| {
                    let id = row.get::<_, String>(0)?;
                    let payload_json = row.get::<_, String>(1)?;
                    let created_at = row.get::<_, i64>(2)?;
                    note_from_event(id, &payload_json, created_at)
                },
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    StorageError::NotFound(format!("research note {note_id}"))
                }
                other => StorageError::Sqlite(other),
            })
    }
}

fn validate_research_note_request(request: &ResearchNoteRequest) -> Result<(), StorageError> {
    validate_non_empty("problem", &request.problem)?;
    validate_non_empty("question", &request.question)?;
    validate_non_empty("finding", &request.finding)?;
    match request.confidence.as_str() {
        "low" | "medium" | "high" => Ok(()),
        other => Err(StorageError::InvalidInput(format!(
            "confidence must be low, medium, or high; got {other}"
        ))),
    }
}

fn validate_non_empty(label: &str, value: &str) -> Result<(), StorageError> {
    if value.trim().is_empty() {
        Err(StorageError::InvalidInput(format!(
            "research note {label} must not be empty"
        )))
    } else {
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ResearchNoteJson {
    id: String,
    problem: String,
    question: String,
    finding: String,
    confidence: String,
    source: Option<String>,
    created_at: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ResearchNotePayload {
    #[serde(default)]
    problem: String,
    #[serde(default)]
    question: String,
    #[serde(default)]
    finding: String,
    #[serde(default)]
    confidence: String,
    #[serde(default)]
    source: Option<String>,
}

fn research_note_payload_json(request: &ResearchNoteRequest) -> String {
    serde_json::to_string(&ResearchNotePayload {
        problem: request.problem.trim().to_string(),
        question: request.question.trim().to_string(),
        finding: request.finding.trim().to_string(),
        confidence: request.confidence.trim().to_string(),
        source: trimmed_non_empty(request.source.as_deref()),
    })
    .expect("research note payload serializes to JSON")
}

fn note_from_event(
    id: String,
    payload_json: &str,
    created_at: i64,
) -> Result<ResearchNote, rusqlite::Error> {
    let payload: ResearchNotePayload = serde_json::from_str(payload_json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(err))
    })?;
    Ok(ResearchNote {
        id,
        problem: payload.problem,
        question: payload.question,
        finding: payload.finding,
        confidence: payload.confidence,
        source: payload.source,
        created_at,
    })
}

fn trimmed_non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|inner| !inner.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::storage::ProjectStore;

    use super::{ResearchNote, ResearchNoteJson, ResearchNotePayload, ResearchNoteRequest};

    fn temp_project_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentflow-research-{test_name}-{}-{}",
            std::process::id(),
            crate::storage::now_unix_seconds()
        ))
    }

    #[test]
    fn records_lists_and_inspects_research_notes() {
        let path = temp_project_path("record");
        let store = ProjectStore::init(&path, Some("Research Demo")).unwrap();
        let note = store
            .record_research_note(ResearchNoteRequest {
                problem: "Marker did not validate".to_string(),
                question: "Should homolog genes be considered?".to_string(),
                finding: "A homolog appears in the candidate pathway.".to_string(),
                confidence: "medium".to_string(),
                source: Some("local project notes".to_string()),
            })
            .unwrap();

        assert!(note.id.starts_with("event_"));
        assert_eq!(note.confidence, "medium");
        assert_eq!(note.source.as_deref(), Some("local project notes"));

        let notes = store.list_research_notes().unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].question, "Should homolog genes be considered?");

        let inspected = store.inspect_research_note(&note.id).unwrap();
        assert_eq!(
            inspected.finding,
            "A homolog appears in the candidate pathway."
        );
        assert!(inspected.to_json().contains("Marker did not validate"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn rejects_invalid_research_confidence() {
        let path = temp_project_path("invalid-confidence");
        let store = ProjectStore::init(&path, Some("Research Demo")).unwrap();
        let error = store
            .record_research_note(ResearchNoteRequest {
                problem: "P".to_string(),
                question: "Q".to_string(),
                finding: "F".to_string(),
                confidence: "certain".to_string(),
                source: None,
            })
            .unwrap_err();

        assert!(error.to_string().contains("confidence must be"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn research_note_json_is_exact_byte() {
        let note = ResearchNote {
            id: "event_1".to_string(),
            problem: "Marker \"A\"".to_string(),
            question: "Does TP53\\EGFR validate?".to_string(),
            finding: "Line one\nLine two".to_string(),
            confidence: "high".to_string(),
            source: Some("lab\tbook".to_string()),
            created_at: 42,
        };

        assert_eq!(
            note.to_json(),
            "{\"id\":\"event_1\",\"problem\":\"Marker \\\"A\\\"\",\"question\":\"Does TP53\\\\EGFR validate?\",\"finding\":\"Line one\\nLine two\",\"confidence\":\"high\",\"source\":\"lab\\tbook\",\"created_at\":42}"
        );

        let payload: ResearchNoteJson = serde_json::from_str(&note.to_json()).unwrap();
        assert_eq!(payload.id, "event_1");
    }

    #[test]
    fn research_note_payload_reads_old_handwritten_json() {
        let payload: ResearchNotePayload = serde_json::from_str(
            "{\"problem\":\"P\\\"1\",\"question\":\"Q\\\\2\",\"finding\":\"F\\n3\",\"confidence\":\"medium\",\"source\":null}",
        )
        .unwrap();

        assert_eq!(payload.problem, "P\"1");
        assert_eq!(payload.question, "Q\\2");
        assert_eq!(payload.finding, "F\n3");
        assert_eq!(payload.confidence, "medium");
        assert_eq!(payload.source, None);
    }
}
