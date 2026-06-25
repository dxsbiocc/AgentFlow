use std::fmt;

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::argument::{EvidenceGrade, EvidenceLink, EvidenceLinkRequest, Stance};
use crate::storage::{EventRecord, ProjectStore, StorageError};

const FORAGE_ACTION_STARTED_EVENT: &str = "forage.action_started";
const FORAGE_OBSERVATION_RECORDED_EVENT: &str = "forage.observation_recorded";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessStatus {
    MetadataOnly,
    AbstractAvailable,
    OpenAccessFullText,
    UserProvidedFullText,
    SubscriptionConnectorFullText,
    FullTextUnavailable,
    RetrievalFailed,
}

impl AccessStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MetadataOnly => "metadata_only",
            Self::AbstractAvailable => "abstract_available",
            Self::OpenAccessFullText => "open_access_full_text",
            Self::UserProvidedFullText => "user_provided_full_text",
            Self::SubscriptionConnectorFullText => "subscription_connector_full_text",
            Self::FullTextUnavailable => "full_text_unavailable",
            Self::RetrievalFailed => "retrieval_failed",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "metadata_only" => Some(Self::MetadataOnly),
            "abstract_available" => Some(Self::AbstractAvailable),
            "open_access_full_text" => Some(Self::OpenAccessFullText),
            "user_provided_full_text" => Some(Self::UserProvidedFullText),
            "subscription_connector_full_text" => Some(Self::SubscriptionConnectorFullText),
            "full_text_unavailable" => Some(Self::FullTextUnavailable),
            "retrieval_failed" => Some(Self::RetrievalFailed),
            _ => None,
        }
    }
}

impl fmt::Display for AccessStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForageAction {
    ReadMap,
    ExploreUnknown,
    VerifyKnown,
}

impl ForageAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadMap => "read_map",
            Self::ExploreUnknown => "explore_unknown",
            Self::VerifyKnown => "verify_known",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "read_map" => Some(Self::ReadMap),
            "explore_unknown" => Some(Self::ExploreUnknown),
            "verify_known" => Some(Self::VerifyKnown),
            _ => None,
        }
    }
}

impl fmt::Display for ForageAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForageObservation {
    pub id: String,
    pub source_id: String,
    pub external_id: String,
    pub title: String,
    pub access_status: AccessStatus,
    pub retrieved_at: i64,
}

impl ForageObservation {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("forage observation serializes to JSON")
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ForagePolicy {
    pub explore_enabled: bool,
}

pub fn grade_from_access(status: AccessStatus) -> EvidenceGrade {
    match status {
        AccessStatus::OpenAccessFullText
        | AccessStatus::UserProvidedFullText
        | AccessStatus::SubscriptionConnectorFullText => EvidenceGrade::LiteratureSupported,
        AccessStatus::AbstractAvailable => EvidenceGrade::Hypothesis,
        AccessStatus::MetadataOnly
        | AccessStatus::FullTextUnavailable
        | AccessStatus::RetrievalFailed => EvidenceGrade::Unsupported,
    }
}

/// Preprint / non-peer-reviewed servers, matched case-insensitively as a
/// substring of the source id (e.g. `biorxiv`, `bioRxiv:2024.01`). Full text
/// from these is genuine literature but has not cleared peer review.
const PREPRINT_SOURCES: &[&str] = &[
    "biorxiv",
    "medrxiv",
    "arxiv",
    "chemrxiv",
    "techrxiv",
    "researchsquare",
    "research square",
    "research_square",
    "ssrn",
    "preprints.org",
    "preprint",
    "osf.io",
    "osf preprints",
    "zenodo",
];

/// Whether a foraged source is a known preprint / non-peer-reviewed server.
pub fn is_preprint_source(source_id: &str) -> bool {
    let lowered = source_id.to_ascii_lowercase();
    PREPRINT_SOURCES.iter().any(|name| lowered.contains(name))
}

/// Evidence grade for a foraged source, honest about peer-review status.
///
/// Broadening evidence beyond peer-reviewed databases (e.g. bioRxiv/medRxiv)
/// must not inflate confidence: a preprint's full text is capped at
/// `Hypothesis` — it can support a hypothesis but never earns the peer-reviewed
/// `LiteratureSupported` grade. Non-preprint sources keep their access-based
/// grade unchanged.
pub fn grade_for_forage_source(status: AccessStatus, source_id: &str) -> EvidenceGrade {
    let grade = grade_from_access(status);
    if grade == EvidenceGrade::LiteratureSupported && is_preprint_source(source_id) {
        EvidenceGrade::Hypothesis
    } else {
        grade
    }
}

pub fn current_strength(strength0: f64, age_days: f64, half_life_days: u32) -> f64 {
    if half_life_days == 0 || age_days <= 0.0 {
        return strength0;
    }
    strength0 * 0.5_f64.powf(age_days / f64::from(half_life_days))
}

impl ProjectStore {
    pub fn record_forage_action(
        &self,
        action: ForageAction,
        query: &str,
        source_id: &str,
    ) -> Result<String, StorageError> {
        validate_non_empty("action query", query)?;
        validate_non_empty("action source_id", source_id)?;

        let id = self.append_event(EventRecord {
            flow_id: None,
            step_id: None,
            run_id: None,
            event_type: FORAGE_ACTION_STARTED_EVENT.to_string(),
            payload_json: forage_action_payload_json(action, query, source_id),
        })?;
        self.touch_project()?;
        Ok(id)
    }

    pub fn record_forage_observation(
        &self,
        source_id: &str,
        external_id: &str,
        title: &str,
        access_status: AccessStatus,
    ) -> Result<ForageObservation, StorageError> {
        validate_non_empty("observation source_id", source_id)?;
        validate_non_empty("observation external_id", external_id)?;
        validate_non_empty("observation title", title)?;

        let id = self.append_event(EventRecord {
            flow_id: None,
            step_id: None,
            run_id: None,
            event_type: FORAGE_OBSERVATION_RECORDED_EVENT.to_string(),
            payload_json: forage_observation_payload_json(
                source_id,
                external_id,
                title,
                access_status,
            ),
        })?;
        self.touch_project()?;
        self.inspect_forage_observation(&id)
    }

    pub fn list_forage_observations(&self) -> Result<Vec<ForageObservation>, StorageError> {
        let reverted = self.reverted_event_id_set()?;
        let mut stmt = self.connection().prepare(
            "SELECT id, payload_json, created_at
             FROM events
             WHERE event_type = ?1
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map(params![FORAGE_OBSERVATION_RECORDED_EVENT], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;

        let mut observations = Vec::new();
        for row in rows {
            let (event_id, payload_json, created_at) = row?;
            if reverted.contains(&event_id) {
                continue;
            }
            observations.push(forage_observation_from_event(
                event_id,
                &payload_json,
                created_at,
            )?);
        }
        Ok(observations)
    }

    pub fn inspect_forage_observation(&self, id: &str) -> Result<ForageObservation, StorageError> {
        if id.trim().is_empty() {
            return Err(StorageError::InvalidInput(
                "forage observation id must not be empty".to_string(),
            ));
        }
        let id = id.trim();
        if self.reverted_event_id_set()?.contains(id) {
            return Err(StorageError::NotFound(format!("forage observation {id}")));
        }
        let row = self
            .connection()
            .query_row(
                "SELECT id, payload_json, created_at
                 FROM events
                 WHERE id = ?1 AND event_type = ?2",
                params![id, FORAGE_OBSERVATION_RECORDED_EVENT],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    StorageError::NotFound(format!("forage observation {id}"))
                }
                other => StorageError::Sqlite(other),
            })?;

        forage_observation_from_event(row.0, &row.1, row.2)
    }

    pub fn link_forage_evidence(
        &self,
        hypothesis_id: &str,
        forage_observation_id: &str,
        stance: Stance,
        note: &str,
    ) -> Result<EvidenceLink, StorageError> {
        let observation = self.inspect_forage_observation(forage_observation_id)?;
        self.link_evidence(EvidenceLinkRequest {
            hypothesis_id: hypothesis_id.to_string(),
            observation_id: Some(observation.id.clone()),
            source: Some(observation.external_id.clone()),
            grade: grade_for_forage_source(observation.access_status, &observation.source_id),
            stance,
            note: note.to_string(),
        })
    }
}

fn validate_non_empty(label: &str, value: &str) -> Result<(), StorageError> {
    if value.trim().is_empty() {
        Err(StorageError::InvalidInput(format!(
            "forage {label} must not be empty"
        )))
    } else {
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ForageActionPayload {
    action: ForageAction,
    query: String,
    source_id: String,
}

fn forage_action_payload_json(action: ForageAction, query: &str, source_id: &str) -> String {
    serde_json::to_string(&ForageActionPayload {
        action,
        query: query.trim().to_string(),
        source_id: source_id.trim().to_string(),
    })
    .expect("forage action payload serializes to JSON")
}

#[derive(Debug, Serialize, Deserialize)]
struct ForageObservationPayload {
    source_id: String,
    external_id: String,
    title: String,
    access_status: AccessStatus,
}

fn forage_observation_payload_json(
    source_id: &str,
    external_id: &str,
    title: &str,
    access_status: AccessStatus,
) -> String {
    serde_json::to_string(&ForageObservationPayload {
        source_id: source_id.trim().to_string(),
        external_id: external_id.trim().to_string(),
        title: title.trim().to_string(),
        access_status,
    })
    .expect("forage observation payload serializes to JSON")
}

fn forage_observation_from_event(
    id: String,
    payload_json: &str,
    created_at: i64,
) -> Result<ForageObservation, StorageError> {
    let payload = forage_observation_payload_from_json(&id, payload_json)?;
    Ok(ForageObservation {
        id,
        source_id: payload.source_id,
        external_id: payload.external_id,
        title: payload.title,
        access_status: payload.access_status,
        retrieved_at: created_at,
    })
}

fn forage_observation_payload_from_json(
    event_id: &str,
    payload_json: &str,
) -> Result<ForageObservationPayload, StorageError> {
    serde_json::from_str(payload_json).map_err(|err| {
        StorageError::InvalidInput(format!(
            "forage event {event_id} has invalid payload: {err}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::argument::{EvidenceGrade, InconclusiveKind, RuleBasedEngine, Stance, Verdict};
    use crate::hypothesis::HypothesisRequest;
    use crate::storage::{now_unix_seconds, EventRecord, ProjectStore, StorageError};

    use super::{
        current_strength, grade_for_forage_source, grade_from_access, is_preprint_source,
        AccessStatus, ForageAction, ForagePolicy,
    };

    fn temp_project_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentflow-forage-{test_name}-{}-{}",
            std::process::id(),
            now_unix_seconds()
        ))
    }

    fn record_hypothesis(store: &ProjectStore) -> String {
        store
            .record_hypothesis(HypothesisRequest {
                statement: "External literature supports marker A".to_string(),
                origin: "agent".to_string(),
                related_goal_id: "goal_forage".to_string(),
            })
            .unwrap()
            .id
    }

    fn forage_event_types(store: &ProjectStore) -> Vec<String> {
        let mut stmt = store
            .connection()
            .prepare(
                "SELECT event_type FROM events
                 WHERE event_type LIKE 'forage.%'
                 ORDER BY created_at ASC, id ASC",
            )
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    #[test]
    fn access_status_maps_to_compliant_evidence_grades() {
        let cases = [
            (AccessStatus::MetadataOnly, EvidenceGrade::Unsupported),
            (AccessStatus::AbstractAvailable, EvidenceGrade::Hypothesis),
            (
                AccessStatus::OpenAccessFullText,
                EvidenceGrade::LiteratureSupported,
            ),
            (
                AccessStatus::UserProvidedFullText,
                EvidenceGrade::LiteratureSupported,
            ),
            (
                AccessStatus::SubscriptionConnectorFullText,
                EvidenceGrade::LiteratureSupported,
            ),
            (
                AccessStatus::FullTextUnavailable,
                EvidenceGrade::Unsupported,
            ),
            (AccessStatus::RetrievalFailed, EvidenceGrade::Unsupported),
        ];

        for (status, grade) in cases {
            assert_eq!(grade_from_access(status), grade);
            assert_eq!(AccessStatus::parse(status.as_str()), Some(status));
            assert_eq!(status.to_string(), status.as_str());
        }
        assert_eq!(AccessStatus::parse("full_text"), None);
    }

    #[test]
    fn preprint_full_text_is_capped_below_peer_reviewed() {
        // Recognized preprint servers, in mixed case and with id suffixes.
        for source in [
            "biorxiv",
            "bioRxiv:2024.01.02.573210",
            "medRxiv",
            "arXiv:2401.00001",
            "researchsquare",
            "ssrn",
            "preprints.org",
        ] {
            assert!(is_preprint_source(source), "{source} should be a preprint");
            // Full text from a preprint can support a hypothesis but is never
            // peer-reviewed `LiteratureSupported`.
            assert_eq!(
                grade_for_forage_source(AccessStatus::OpenAccessFullText, source),
                EvidenceGrade::Hypothesis,
                "{source} full text must be capped"
            );
            assert_eq!(
                grade_for_forage_source(AccessStatus::UserProvidedFullText, source),
                EvidenceGrade::Hypothesis
            );
            // Weaker access stays exactly where access alone puts it.
            assert_eq!(
                grade_for_forage_source(AccessStatus::AbstractAvailable, source),
                EvidenceGrade::Hypothesis
            );
            assert_eq!(
                grade_for_forage_source(AccessStatus::MetadataOnly, source),
                EvidenceGrade::Unsupported
            );
        }

        // Peer-reviewed / unknown sources keep the access-based grade.
        for source in [
            "pubmed",
            "pmc",
            "doi:10.1038/s41586-024-00001",
            "cbioportal",
        ] {
            assert!(!is_preprint_source(source), "{source} is not a preprint");
            assert_eq!(
                grade_for_forage_source(AccessStatus::OpenAccessFullText, source),
                EvidenceGrade::LiteratureSupported,
                "{source} full text keeps peer-reviewed grade"
            );
        }
    }

    #[test]
    fn forage_action_round_trips_payload_text() {
        for action in [
            ForageAction::ReadMap,
            ForageAction::ExploreUnknown,
            ForageAction::VerifyKnown,
        ] {
            assert_eq!(ForageAction::parse(action.as_str()), Some(action));
            assert_eq!(action.to_string(), action.as_str());
        }
        assert_eq!(ForageAction::parse("random_walk"), None);
    }

    #[test]
    fn enum_json_strings_match_display_contract() {
        for status in [
            AccessStatus::MetadataOnly,
            AccessStatus::AbstractAvailable,
            AccessStatus::OpenAccessFullText,
            AccessStatus::UserProvidedFullText,
            AccessStatus::SubscriptionConnectorFullText,
            AccessStatus::FullTextUnavailable,
            AccessStatus::RetrievalFailed,
        ] {
            assert_eq!(
                serde_json::to_string(&status).unwrap(),
                format!("\"{}\"", status.as_str())
            );
        }
        for action in [
            ForageAction::ReadMap,
            ForageAction::ExploreUnknown,
            ForageAction::VerifyKnown,
        ] {
            assert_eq!(
                serde_json::to_string(&action).unwrap(),
                format!("\"{}\"", action.as_str())
            );
        }
    }

    #[test]
    fn current_strength_applies_half_life_decay() {
        assert!((current_strength(10.0, 30.0, 30) - 5.0).abs() < f64::EPSILON);
        assert_eq!(current_strength(10.0, 0.0, 30), 10.0);
        assert_eq!(current_strength(10.0, -1.0, 30), 10.0);
        assert_eq!(current_strength(10.0, 365.0, 0), 10.0);
    }

    #[test]
    fn default_policy_disables_exploration() {
        assert!(!ForagePolicy::default().explore_enabled);
        assert!(
            ForagePolicy {
                explore_enabled: true
            }
            .explore_enabled
        );
    }

    #[test]
    fn records_action_and_observation_without_new_event_types() {
        let path = temp_project_path("record");
        let store = ProjectStore::init(&path, Some("Forage Demo")).unwrap();

        let action_id = store
            .record_forage_action(ForageAction::ReadMap, " marker A pathway B ", " pubmed ")
            .unwrap();
        let observation = store
            .record_forage_observation(
                " pubmed ",
                " PMID:123 ",
                " Marker A supports pathway B ",
                AccessStatus::OpenAccessFullText,
            )
            .unwrap();

        assert!(action_id.starts_with("event_"));
        assert!(observation.id.starts_with("event_"));
        assert_eq!(observation.source_id, "pubmed");
        assert_eq!(observation.external_id, "PMID:123");
        assert_eq!(observation.title, "Marker A supports pathway B");
        assert_eq!(observation.access_status, AccessStatus::OpenAccessFullText);
        assert!(observation.retrieved_at > 0);

        let observations = store.list_forage_observations().unwrap();
        assert_eq!(observations, vec![observation.clone()]);
        assert_eq!(
            store.inspect_forage_observation(&observation.id).unwrap(),
            observation
        );
        assert_eq!(
            forage_event_types(&store),
            vec!["forage.action_started", "forage.observation_recorded"]
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn legacy_handwritten_payloads_parse_with_json_whitespace_and_ordering() {
        let path = temp_project_path("legacy-payload");
        let store = ProjectStore::init(&path, Some("Forage Demo")).unwrap();
        let event_id = store
            .append_event(EventRecord {
                flow_id: None,
                step_id: None,
                run_id: None,
                event_type: super::FORAGE_OBSERVATION_RECORDED_EVENT.to_string(),
                payload_json: r#"{
                    "access_status": "abstract_available",
                    "title": "Legacy \"payload\"\nparses",
                    "external_id": "PMID:legacy",
                    "source_id": "pubmed"
                }"#
                .to_string(),
            })
            .unwrap();

        let inspected = store.inspect_forage_observation(&event_id).unwrap();
        assert_eq!(inspected.source_id, "pubmed");
        assert_eq!(inspected.external_id, "PMID:legacy");
        assert_eq!(inspected.title, "Legacy \"payload\"\nparses");
        assert_eq!(inspected.access_status, AccessStatus::AbstractAvailable);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn json_outputs_match_legacy_bytes() {
        let observation = super::ForageObservation {
            id: "event_1".to_string(),
            source_id: "pubmed".to_string(),
            external_id: "PMID:123".to_string(),
            title: "Quote \" and newline\nslash \\ tab\t".to_string(),
            access_status: AccessStatus::AbstractAvailable,
            retrieved_at: 123,
        };

        assert_eq!(
            observation.to_json(),
            "{\"id\":\"event_1\",\"source_id\":\"pubmed\",\"external_id\":\"PMID:123\",\"title\":\"Quote \\\" and newline\\nslash \\\\ tab\\t\",\"access_status\":\"abstract_available\",\"retrieved_at\":123}"
        );
        assert_eq!(
            super::forage_action_payload_json(
                ForageAction::ExploreUnknown,
                " query \"x\" ",
                " pubmed "
            ),
            "{\"action\":\"explore_unknown\",\"query\":\"query \\\"x\\\"\",\"source_id\":\"pubmed\"}"
        );
        assert_eq!(
            super::forage_observation_payload_json(
                " pubmed ",
                " PMID:123 ",
                " Title\nwith tab\t ",
                AccessStatus::OpenAccessFullText,
            ),
            "{\"source_id\":\"pubmed\",\"external_id\":\"PMID:123\",\"title\":\"Title\\nwith tab\",\"access_status\":\"open_access_full_text\"}"
        );
    }

    #[test]
    fn reverted_forage_observation_is_hidden_from_projection() {
        let path = temp_project_path("reverted-observation");
        let store = ProjectStore::init(&path, Some("Forage Demo")).unwrap();
        let checkpoint = store.create_checkpoint("before-observation").unwrap();
        let observation = store
            .record_forage_observation(
                "pubmed",
                "PMID:789",
                "Later paper",
                AccessStatus::AbstractAvailable,
            )
            .unwrap();

        assert_eq!(store.list_forage_observations().unwrap().len(), 1);
        assert_eq!(
            store.inspect_forage_observation(&observation.id).unwrap(),
            observation
        );

        store.revert_to(&checkpoint.id).unwrap();

        assert!(store.list_forage_observations().unwrap().is_empty());
        assert!(matches!(
            store
                .inspect_forage_observation(&observation.id)
                .unwrap_err(),
            StorageError::NotFound(_)
        ));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn observation_projection_preserves_escaped_text() {
        let path = temp_project_path("escaped");
        let store = ProjectStore::init(&path, Some("Forage Demo")).unwrap();
        let observation = store
            .record_forage_observation(
                "local",
                "doi:10.1/example",
                "Quoted \"title\"\nwith slash \\ marker",
                AccessStatus::AbstractAvailable,
            )
            .unwrap();

        let inspected = store.inspect_forage_observation(&observation.id).unwrap();
        assert_eq!(
            inspected.title,
            "Quoted \"title\"\nwith slash \\ marker".to_string()
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn rejects_empty_forage_inputs_and_missing_observation() {
        let path = temp_project_path("reject");
        let store = ProjectStore::init(&path, Some("Forage Demo")).unwrap();

        let error = store
            .record_forage_action(ForageAction::ReadMap, " ", "pubmed")
            .unwrap_err();
        assert!(error.to_string().contains("forage action query"));

        let error = store
            .record_forage_observation(
                "pubmed",
                " ",
                "Some title",
                AccessStatus::OpenAccessFullText,
            )
            .unwrap_err();
        assert!(error.to_string().contains("forage observation external_id"));

        let error = store.inspect_forage_observation("missing").unwrap_err();
        assert!(error.to_string().contains("forage observation missing"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn links_forage_observation_into_evidence_ledger() {
        let path = temp_project_path("link");
        let store = ProjectStore::init(&path, Some("Forage Demo")).unwrap();
        let hypothesis_id = record_hypothesis(&store);
        let observation = store
            .record_forage_observation(
                "biorxiv",
                "doi:10.1101/2026.01.01.123456",
                "Full-text preprint",
                AccessStatus::UserProvidedFullText,
            )
            .unwrap();

        let link = store
            .link_forage_evidence(
                &hypothesis_id,
                &observation.id,
                Stance::Supports,
                "Full text supports the marker relationship",
            )
            .unwrap();

        assert_eq!(link.hypothesis_id, hypothesis_id);
        assert_eq!(
            link.observation_id.as_deref(),
            Some(observation.id.as_str())
        );
        assert_eq!(
            link.source.as_deref(),
            Some("doi:10.1101/2026.01.01.123456")
        );
        // A bioRxiv preprint's full text is genuine literature but not
        // peer-reviewed, so it is capped at Hypothesis rather than the
        // peer-reviewed LiteratureSupported grade.
        assert_eq!(link.grade, EvidenceGrade::Hypothesis);
        assert_eq!(link.stance, Stance::Supports);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn abstract_forage_evidence_cannot_affirm_verdict() {
        let path = temp_project_path("abstract-verdict");
        let store = ProjectStore::init(&path, Some("Forage Demo")).unwrap();
        let hypothesis_id = record_hypothesis(&store);
        let observation = store
            .record_forage_observation(
                "pubmed",
                "PMID:456",
                "Abstract-only paper",
                AccessStatus::AbstractAvailable,
            )
            .unwrap();
        store
            .link_forage_evidence(
                &hypothesis_id,
                &observation.id,
                Stance::Supports,
                "Abstract triage signal only",
            )
            .unwrap();

        let report = store
            .render_verdict(&hypothesis_id, &RuleBasedEngine, None)
            .unwrap();

        assert!(matches!(
            report.verdict,
            Verdict::Inconclusive(InconclusiveKind::Provisional { .. })
        ));
        assert_ne!(report.verdict, Verdict::Affirmed);
        assert_eq!(report.supporting[0].grade, EvidenceGrade::Hypothesis);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn literature_supported_forage_evidence_alone_cannot_affirm_verdict() {
        let path = temp_project_path("literature-verdict");
        let store = ProjectStore::init(&path, Some("Forage Demo")).unwrap();
        let hypothesis_id = record_hypothesis(&store);

        for index in 0..3 {
            let observation = store
                .record_forage_observation(
                    "pubmed",
                    &format!("PMID:{index}"),
                    &format!("Full text paper {index}"),
                    AccessStatus::OpenAccessFullText,
                )
                .unwrap();
            store
                .link_forage_evidence(
                    &hypothesis_id,
                    &observation.id,
                    Stance::Supports,
                    "Full text supports the hypothesis",
                )
                .unwrap();
        }

        let report = store
            .render_verdict(&hypothesis_id, &RuleBasedEngine, None)
            .unwrap();

        assert_ne!(report.verdict, Verdict::Affirmed);
        assert!(matches!(
            report.verdict,
            Verdict::Inconclusive(InconclusiveKind::Provisional { .. })
        ));
        assert_eq!(report.supporting.len(), 3);
        assert!(report
            .supporting
            .iter()
            .all(|link| link.grade == EvidenceGrade::LiteratureSupported));

        let _ = std::fs::remove_dir_all(path);
    }
}
