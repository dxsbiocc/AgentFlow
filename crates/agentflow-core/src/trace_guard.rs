use std::collections::{BTreeSet, HashSet};

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::storage::{EventRecord, ProjectStore, StorageError};

const TRACE_CHECKPOINT_CREATED_EVENT: &str = "trace.checkpoint_created";
const TRACE_REVERTED_EVENT: &str = "trace.reverted";
const DRIFT_SURFACE_THRESHOLD: u32 = 5;
const AUTONOMOUS_EVENT_TYPE_COUNT: usize = 5;
const AUTONOMOUS_EVENT_TYPES: [&str; AUTONOMOUS_EVENT_TYPE_COUNT] = [
    "hypothesis.transitioned",
    "argument.verdict_rendered",
    "argument.evidence_linked",
    "graph_patch_proposed",
    "handoff.decision_point_raised",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: String,
    pub horizon_event_id: Option<String>,
    pub label: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriftReport {
    pub from_checkpoint: String,
    pub net_goal_delta: String,
    pub autonomous_steps: u32,
    pub should_surface: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevertRecord {
    pub id: String,
    pub checkpoint_id: String,
    pub reverted_event_ids: Vec<String>,
    pub created_at: i64,
}

impl Checkpoint {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("checkpoint serializes to JSON")
    }
}

impl DriftReport {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("drift report serializes to JSON")
    }
}

impl RevertRecord {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("revert record serializes to JSON")
    }
}

impl ProjectStore {
    pub fn create_checkpoint(&self, label: &str) -> Result<Checkpoint, StorageError> {
        let label = validate_non_empty("checkpoint label", label)?;
        let horizon_event_id = self.last_event_id()?;
        let id = self.append_event(EventRecord {
            flow_id: None,
            step_id: None,
            run_id: None,
            event_type: TRACE_CHECKPOINT_CREATED_EVENT.to_string(),
            payload_json: checkpoint_payload_json(label, horizon_event_id.as_deref()),
        })?;
        self.touch_project()?;
        self.inspect_checkpoint(&id)
    }

    pub fn list_checkpoints(&self) -> Result<Vec<Checkpoint>, StorageError> {
        let mut stmt = self.connection().prepare(
            "SELECT id, payload_json, created_at
             FROM events
             WHERE event_type = ?1
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([TRACE_CHECKPOINT_CREATED_EVENT], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;

        let mut checkpoints = Vec::new();
        for row in rows {
            let (id, payload_json, created_at) = row?;
            checkpoints.push(checkpoint_from_event(id, &payload_json, created_at)?);
        }
        Ok(checkpoints)
    }

    pub fn inspect_checkpoint(&self, id: &str) -> Result<Checkpoint, StorageError> {
        let id = validate_non_empty("checkpoint id", id)?;
        let row = self
            .connection()
            .query_row(
                "SELECT id, payload_json, created_at
                 FROM events
                 WHERE id = ?1 AND event_type = ?2",
                params![id, TRACE_CHECKPOINT_CREATED_EVENT],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound(format!("checkpoint {id}")))?;
        checkpoint_from_event(row.0, &row.1, row.2)
    }

    pub fn detect_drift(&self, checkpoint_id: &str) -> Result<DriftReport, StorageError> {
        let checkpoint = self.inspect_checkpoint(checkpoint_id)?;
        let events = self.events_after_horizon(checkpoint.horizon_event_id.as_deref())?;
        let mut counts = [0_u32; AUTONOMOUS_EVENT_TYPE_COUNT];
        let mut autonomous_steps = 0_u32;

        for (_, event_type) in events {
            if let Some(index) = autonomous_event_type_index(&event_type) {
                counts[index] = counts[index].saturating_add(1);
                autonomous_steps = autonomous_steps.saturating_add(1);
            }
        }

        Ok(DriftReport {
            from_checkpoint: checkpoint.id,
            net_goal_delta: net_goal_delta(&counts),
            autonomous_steps,
            should_surface: autonomous_steps >= DRIFT_SURFACE_THRESHOLD,
        })
    }

    pub fn revert_to(&self, checkpoint_id: &str) -> Result<RevertRecord, StorageError> {
        let checkpoint = self.inspect_checkpoint(checkpoint_id)?;
        let reverted_event_ids = self
            .events_after_horizon(checkpoint.horizon_event_id.as_deref())?
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        let id = self.append_event(EventRecord {
            flow_id: None,
            step_id: None,
            run_id: None,
            event_type: TRACE_REVERTED_EVENT.to_string(),
            payload_json: revert_record_payload_json(&checkpoint.id, &reverted_event_ids),
        })?;
        self.touch_project()?;
        self.inspect_revert_record(&id)
    }

    pub fn reverted_event_ids(&self) -> Result<Vec<String>, StorageError> {
        let mut stmt = self.connection().prepare(
            "SELECT id, payload_json
             FROM events
             WHERE event_type = ?1
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([TRACE_REVERTED_EVENT], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        let mut reverted_ids = BTreeSet::new();
        for row in rows {
            let (event_id, payload_json) = row?;
            let payload = revert_record_payload_from_json(&event_id, &payload_json)?;
            for reverted_id in payload.reverted_event_ids {
                reverted_ids.insert(reverted_id);
            }
        }
        Ok(reverted_ids.into_iter().collect())
    }

    /// 已被回退的事件 id 集合（复用现有 reverted_event_ids，去重为集合）。
    pub fn reverted_event_id_set(&self) -> Result<HashSet<String>, StorageError> {
        Ok(self.reverted_event_ids()?.into_iter().collect())
    }

    fn last_event_id(&self) -> Result<Option<String>, StorageError> {
        self.connection()
            .query_row(
                "SELECT id FROM events ORDER BY created_at DESC, id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(StorageError::from)
    }

    fn events_after_horizon(
        &self,
        horizon_event_id: Option<&str>,
    ) -> Result<Vec<(String, String)>, StorageError> {
        let Some(horizon_event_id) = horizon_event_id else {
            let mut stmt = self.connection().prepare(
                "SELECT id, event_type
                 FROM events
                 ORDER BY created_at ASC, id ASC",
            )?;
            let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            let mut events = Vec::new();
            for row in rows {
                events.push(row?);
            }
            return Ok(events);
        };

        let horizon_created_at = self.event_created_at(horizon_event_id)?;
        let mut stmt = self.connection().prepare(
            "SELECT id, event_type
             FROM events
             WHERE created_at > ?1 OR (created_at = ?1 AND id > ?2)
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map(params![horizon_created_at, horizon_event_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;

        let mut events = Vec::new();
        for row in rows {
            events.push(row?);
        }
        Ok(events)
    }

    fn event_created_at(&self, event_id: &str) -> Result<i64, StorageError> {
        self.connection()
            .query_row(
                "SELECT created_at FROM events WHERE id = ?1",
                [event_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound(format!("event {event_id}")))
    }

    fn inspect_revert_record(&self, id: &str) -> Result<RevertRecord, StorageError> {
        let row = self
            .connection()
            .query_row(
                "SELECT id, payload_json, created_at
                 FROM events
                 WHERE id = ?1 AND event_type = ?2",
                params![id, TRACE_REVERTED_EVENT],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound(format!("revert record {id}")))?;
        revert_record_from_event(row.0, &row.1, row.2)
    }
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
struct CheckpointPayload {
    label: String,
    #[serde(default)]
    horizon_event_id: Option<String>,
}

fn checkpoint_payload_json(label: &str, horizon_event_id: Option<&str>) -> String {
    serde_json::to_string(&CheckpointPayload {
        label: label.to_string(),
        horizon_event_id: horizon_event_id
            .filter(|inner| !inner.trim().is_empty())
            .map(ToString::to_string),
    })
    .expect("checkpoint payload serializes to JSON")
}

fn checkpoint_from_event(
    id: String,
    payload_json: &str,
    created_at: i64,
) -> Result<Checkpoint, StorageError> {
    let payload = checkpoint_payload_from_json(&id, payload_json)?;
    Ok(Checkpoint {
        id,
        horizon_event_id: payload.horizon_event_id,
        label: payload.label,
        created_at,
    })
}

fn checkpoint_payload_from_json(
    event_id: &str,
    payload_json: &str,
) -> Result<CheckpointPayload, StorageError> {
    serde_json::from_str(payload_json).map_err(|err| {
        StorageError::InvalidInput(format!("trace event {event_id} has invalid payload: {err}"))
    })
}

#[derive(Debug, Serialize, Deserialize)]
struct RevertRecordPayload {
    checkpoint_id: String,
    reverted_event_ids: Vec<String>,
}

fn revert_record_payload_json(checkpoint_id: &str, reverted_event_ids: &[String]) -> String {
    serde_json::to_string(&RevertRecordPayload {
        checkpoint_id: checkpoint_id.to_string(),
        reverted_event_ids: reverted_event_ids.to_vec(),
    })
    .expect("revert record payload serializes to JSON")
}

fn revert_record_from_event(
    id: String,
    payload_json: &str,
    created_at: i64,
) -> Result<RevertRecord, StorageError> {
    let payload = revert_record_payload_from_json(&id, payload_json)?;
    Ok(RevertRecord {
        id,
        checkpoint_id: payload.checkpoint_id,
        reverted_event_ids: payload.reverted_event_ids,
        created_at,
    })
}

fn revert_record_payload_from_json(
    event_id: &str,
    payload_json: &str,
) -> Result<RevertRecordPayload, StorageError> {
    serde_json::from_str(payload_json).map_err(|err| {
        StorageError::InvalidInput(format!("trace event {event_id} has invalid payload: {err}"))
    })
}

fn autonomous_event_type_index(event_type: &str) -> Option<usize> {
    AUTONOMOUS_EVENT_TYPES
        .iter()
        .position(|candidate| *candidate == event_type)
}

fn net_goal_delta(counts: &[u32; AUTONOMOUS_EVENT_TYPE_COUNT]) -> String {
    AUTONOMOUS_EVENT_TYPES
        .iter()
        .zip(counts.iter())
        .map(|(event_type, count)| format!("{event_type}={count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::storage::{now_unix_seconds, EventRecord, ProjectStore};

    use super::{
        revert_record_payload_json, AUTONOMOUS_EVENT_TYPES, TRACE_CHECKPOINT_CREATED_EVENT,
        TRACE_REVERTED_EVENT,
    };

    fn temp_project_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentflow-trace-guard-{test_name}-{}-{}",
            std::process::id(),
            now_unix_seconds()
        ))
    }

    fn append_test_event(store: &ProjectStore, event_type: &str) -> String {
        store
            .append_event(EventRecord {
                flow_id: None,
                step_id: None,
                run_id: None,
                event_type: event_type.to_string(),
                payload_json: "{}".to_string(),
            })
            .unwrap()
    }

    fn event_count(store: &ProjectStore) -> i64 {
        store
            .connection()
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn creates_checkpoint_with_empty_horizon() {
        let path = temp_project_path("empty-horizon");
        let store = ProjectStore::init(&path, Some("Trace Demo")).unwrap();

        let checkpoint = store.create_checkpoint(" baseline ").unwrap();

        assert!(checkpoint.id.starts_with("event_"));
        assert_eq!(checkpoint.horizon_event_id, None);
        assert_eq!(checkpoint.label, "baseline");
        assert_eq!(store.list_checkpoints().unwrap(), vec![checkpoint.clone()]);
        assert_eq!(
            store.inspect_checkpoint(&checkpoint.id).unwrap(),
            checkpoint
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn legacy_handwritten_payloads_parse_with_json_whitespace_and_ordering() {
        let path = temp_project_path("legacy-payload");
        let store = ProjectStore::init(&path, Some("Trace Demo")).unwrap();
        let checkpoint_id = store
            .append_event(EventRecord {
                flow_id: None,
                step_id: None,
                run_id: None,
                event_type: TRACE_CHECKPOINT_CREATED_EVENT.to_string(),
                payload_json: r#"{
                    "horizon_event_id": "event_horizon",
                    "label": "Legacy \"checkpoint\"\nlabel"
                }"#
                .to_string(),
            })
            .unwrap();
        store
            .append_event(EventRecord {
                flow_id: None,
                step_id: None,
                run_id: None,
                event_type: TRACE_REVERTED_EVENT.to_string(),
                payload_json: r#"{
                    "reverted_event_ids": ["event_b", "event_a"],
                    "checkpoint_id": "checkpoint_legacy"
                }"#
                .to_string(),
            })
            .unwrap();

        let checkpoint = store.inspect_checkpoint(&checkpoint_id).unwrap();
        assert_eq!(
            checkpoint.horizon_event_id.as_deref(),
            Some("event_horizon")
        );
        assert_eq!(checkpoint.label, "Legacy \"checkpoint\"\nlabel");
        assert_eq!(
            store.reverted_event_ids().unwrap(),
            vec!["event_a".to_string(), "event_b".to_string()]
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn json_outputs_match_legacy_bytes() {
        let checkpoint = super::Checkpoint {
            id: "event_1".to_string(),
            horizon_event_id: Some("event_0".to_string()),
            label: "Quote \" and newline\nslash \\ tab\t".to_string(),
            created_at: 11,
        };
        let checkpoint_without_horizon = super::Checkpoint {
            id: "event_2".to_string(),
            horizon_event_id: None,
            label: "baseline".to_string(),
            created_at: 12,
        };
        let drift = super::DriftReport {
            from_checkpoint: "event_1".to_string(),
            net_goal_delta: "hypothesis.transitioned=1\nargument.evidence_linked=0".to_string(),
            autonomous_steps: 3,
            should_surface: false,
        };
        let revert = super::RevertRecord {
            id: "event_3".to_string(),
            checkpoint_id: "event_1".to_string(),
            reverted_event_ids: vec!["event_2".to_string(), "event_4".to_string()],
            created_at: 13,
        };

        assert_eq!(
            checkpoint.to_json(),
            "{\"id\":\"event_1\",\"horizon_event_id\":\"event_0\",\"label\":\"Quote \\\" and newline\\nslash \\\\ tab\\t\",\"created_at\":11}"
        );
        assert_eq!(
            checkpoint_without_horizon.to_json(),
            "{\"id\":\"event_2\",\"horizon_event_id\":null,\"label\":\"baseline\",\"created_at\":12}"
        );
        assert_eq!(
            drift.to_json(),
            "{\"from_checkpoint\":\"event_1\",\"net_goal_delta\":\"hypothesis.transitioned=1\\nargument.evidence_linked=0\",\"autonomous_steps\":3,\"should_surface\":false}"
        );
        assert_eq!(
            revert.to_json(),
            "{\"id\":\"event_3\",\"checkpoint_id\":\"event_1\",\"reverted_event_ids\":[\"event_2\",\"event_4\"],\"created_at\":13}"
        );
        assert_eq!(
            super::checkpoint_payload_json("Quote \"\n", Some("event_0")),
            "{\"label\":\"Quote \\\"\\n\",\"horizon_event_id\":\"event_0\"}"
        );
        assert_eq!(
            super::checkpoint_payload_json("baseline", None),
            "{\"label\":\"baseline\",\"horizon_event_id\":null}"
        );
        assert_eq!(
            super::revert_record_payload_json(
                "checkpoint_1",
                &["event_2".to_string(), "event_3".to_string()]
            ),
            "{\"checkpoint_id\":\"checkpoint_1\",\"reverted_event_ids\":[\"event_2\",\"event_3\"]}"
        );
    }

    #[test]
    fn checkpoint_horizon_stays_fixed_after_later_events() {
        let path = temp_project_path("stable-horizon");
        let store = ProjectStore::init(&path, Some("Trace Demo")).unwrap();
        let horizon = append_test_event(&store, "argument.evidence_linked");

        let checkpoint = store.create_checkpoint("stable").unwrap();
        append_test_event(&store, "hypothesis.transitioned");

        let inspected = store.inspect_checkpoint(&checkpoint.id).unwrap();
        assert_eq!(
            checkpoint.horizon_event_id.as_deref(),
            Some(horizon.as_str())
        );
        assert_eq!(inspected.horizon_event_id, checkpoint.horizon_event_id);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn detects_drift_only_after_checkpoint_horizon() {
        let path = temp_project_path("drift-after-horizon");
        let store = ProjectStore::init(&path, Some("Trace Demo")).unwrap();
        append_test_event(&store, "hypothesis.transitioned");
        let checkpoint = store.create_checkpoint("drift").unwrap();
        append_test_event(&store, "hypothesis.transitioned");
        append_test_event(&store, "argument.evidence_linked");
        append_test_event(&store, "argument.evidence_linked");
        append_test_event(&store, "research_note_recorded");

        let report = store.detect_drift(&checkpoint.id).unwrap();

        assert_eq!(report.from_checkpoint, checkpoint.id);
        assert_eq!(report.autonomous_steps, 3);
        assert!(!report.should_surface);
        assert!(report.net_goal_delta.contains("hypothesis.transitioned=1"));
        assert!(report.net_goal_delta.contains("argument.evidence_linked=2"));
        assert!(report.net_goal_delta.contains("graph_patch_proposed=0"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn drift_surfaces_at_threshold_boundary() {
        let path = temp_project_path("drift-threshold");
        let store = ProjectStore::init(&path, Some("Trace Demo")).unwrap();
        let checkpoint = store.create_checkpoint("threshold").unwrap();

        for event_type in AUTONOMOUS_EVENT_TYPES.iter().take(4) {
            append_test_event(&store, event_type);
        }
        let below = store.detect_drift(&checkpoint.id).unwrap();
        assert_eq!(below.autonomous_steps, 4);
        assert!(!below.should_surface);

        append_test_event(&store, "handoff.decision_point_raised");
        let at_threshold = store.detect_drift(&checkpoint.id).unwrap();
        assert_eq!(at_threshold.autonomous_steps, 5);
        assert!(at_threshold.should_surface);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn revert_records_interval_and_preserves_event_count_growth() {
        let path = temp_project_path("revert-record");
        let store = ProjectStore::init(&path, Some("Trace Demo")).unwrap();
        append_test_event(&store, "research_note_recorded");
        let checkpoint = store.create_checkpoint("rewind").unwrap();
        let first = append_test_event(&store, "hypothesis.transitioned");
        let second = append_test_event(&store, "argument.verdict_rendered");
        let count_before = event_count(&store);

        let record = store.revert_to(&checkpoint.id).unwrap();
        let count_after = event_count(&store);

        assert_eq!(count_after, count_before + 1);
        assert_eq!(record.checkpoint_id, checkpoint.id);
        assert_eq!(
            record.reverted_event_ids,
            vec![checkpoint.id.clone(), first, second]
        );
        assert!(store.reverted_event_ids().unwrap().contains(&checkpoint.id));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn reverted_event_ids_are_deduplicated() {
        let path = temp_project_path("dedupe");
        let store = ProjectStore::init(&path, Some("Trace Demo")).unwrap();
        store
            .append_event(EventRecord {
                flow_id: None,
                step_id: None,
                run_id: None,
                event_type: TRACE_REVERTED_EVENT.to_string(),
                payload_json: revert_record_payload_json(
                    "checkpoint_a",
                    &["event_a".to_string(), "event_b".to_string()],
                ),
            })
            .unwrap();
        store
            .append_event(EventRecord {
                flow_id: None,
                step_id: None,
                run_id: None,
                event_type: TRACE_REVERTED_EVENT.to_string(),
                payload_json: revert_record_payload_json(
                    "checkpoint_b",
                    &["event_b".to_string(), "event_c".to_string()],
                ),
            })
            .unwrap();

        assert_eq!(
            store.reverted_event_ids().unwrap(),
            vec![
                "event_a".to_string(),
                "event_b".to_string(),
                "event_c".to_string()
            ]
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn reverted_event_id_set_reuses_deduplicated_revert_projection() {
        let path = temp_project_path("set");
        let store = ProjectStore::init(&path, Some("Trace Demo")).unwrap();
        store
            .append_event(EventRecord {
                flow_id: None,
                step_id: None,
                run_id: None,
                event_type: TRACE_REVERTED_EVENT.to_string(),
                payload_json: revert_record_payload_json(
                    "checkpoint_a",
                    &["event_a".to_string(), "event_b".to_string()],
                ),
            })
            .unwrap();
        store
            .append_event(EventRecord {
                flow_id: None,
                step_id: None,
                run_id: None,
                event_type: TRACE_REVERTED_EVENT.to_string(),
                payload_json: revert_record_payload_json(
                    "checkpoint_b",
                    &["event_b".to_string(), "event_c".to_string()],
                ),
            })
            .unwrap();

        let reverted = store.reverted_event_id_set().unwrap();
        assert_eq!(reverted.len(), 3);
        assert!(reverted.contains("event_a"));
        assert!(reverted.contains("event_b"));
        assert!(reverted.contains("event_c"));

        let _ = std::fs::remove_dir_all(path);
    }
}
