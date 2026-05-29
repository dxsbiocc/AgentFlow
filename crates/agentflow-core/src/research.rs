use rusqlite::params;

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
        format!(
            concat!(
                "{{",
                "\"id\":\"{}\",",
                "\"problem\":\"{}\",",
                "\"question\":\"{}\",",
                "\"finding\":\"{}\",",
                "\"confidence\":\"{}\",",
                "\"source\":{},",
                "\"created_at\":{}",
                "}}"
            ),
            escape_json(&self.id),
            escape_json(&self.problem),
            escape_json(&self.question),
            escape_json(&self.finding),
            escape_json(&self.confidence),
            optional_json_string(self.source.as_deref()),
            self.created_at
        )
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

fn research_note_payload_json(request: &ResearchNoteRequest) -> String {
    format!(
        concat!(
            "{{",
            "\"problem\":\"{}\",",
            "\"question\":\"{}\",",
            "\"finding\":\"{}\",",
            "\"confidence\":\"{}\",",
            "\"source\":{}",
            "}}"
        ),
        escape_json(request.problem.trim()),
        escape_json(request.question.trim()),
        escape_json(request.finding.trim()),
        escape_json(request.confidence.trim()),
        optional_json_string(request.source.as_deref().map(str::trim))
    )
}

fn note_from_event(
    id: String,
    payload_json: &str,
    created_at: i64,
) -> Result<ResearchNote, rusqlite::Error> {
    Ok(ResearchNote {
        id,
        problem: json_string_field(payload_json, "problem").unwrap_or_default(),
        question: json_string_field(payload_json, "question").unwrap_or_default(),
        finding: json_string_field(payload_json, "finding").unwrap_or_default(),
        confidence: json_string_field(payload_json, "confidence").unwrap_or_default(),
        source: json_nullable_string_field(payload_json, "source"),
        created_at,
    })
}

fn optional_json_string(value: Option<&str>) -> String {
    value.filter(|inner| !inner.trim().is_empty()).map_or_else(
        || "null".to_string(),
        |inner| format!("\"{}\"", escape_json(inner)),
    )
}

fn json_string_field(json: &str, field: &str) -> Option<String> {
    let marker = format!("\"{field}\":\"");
    let start = json.find(&marker)? + marker.len();
    let rest = &json[start..];
    let end = find_json_string_end(rest)?;
    Some(unescape_json_string(&rest[..end]))
}

fn json_nullable_string_field(json: &str, field: &str) -> Option<String> {
    json_string_field(json, field)
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

    use crate::storage::ProjectStore;

    use super::ResearchNoteRequest;

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
}
