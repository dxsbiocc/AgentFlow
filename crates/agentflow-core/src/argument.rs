use std::fmt;

use rusqlite::params;

use crate::hypothesis::Confidence;
use crate::storage::{EventRecord, ProjectStore, StorageError};

const EVIDENCE_LINKED_EVENT: &str = "argument.evidence_linked";
const VERDICT_RENDERED_EVENT: &str = "argument.verdict_rendered";
const AFFIRM_MARGIN: i32 = 3;
const REFUTE_MARGIN: i32 = 3;
const STRONG_MARGIN: i32 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EvidenceGrade {
    Observed,
    Inferred,
    LiteratureSupported,
    Hypothesis,
    Unsupported,
}

impl EvidenceGrade {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::Inferred => "inferred",
            Self::LiteratureSupported => "literature_supported",
            Self::Hypothesis => "hypothesis",
            Self::Unsupported => "unsupported",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "observed" => Some(Self::Observed),
            "inferred" => Some(Self::Inferred),
            "literature_supported" => Some(Self::LiteratureSupported),
            "hypothesis" => Some(Self::Hypothesis),
            "unsupported" => Some(Self::Unsupported),
            _ => None,
        }
    }

    pub fn weight(self) -> i32 {
        match self {
            Self::Observed => 3,
            Self::Inferred => 2,
            Self::LiteratureSupported => 1,
            Self::Hypothesis | Self::Unsupported => 0,
        }
    }
}

impl fmt::Display for EvidenceGrade {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stance {
    Supports,
    Contradicts,
    Neutral,
}

impl Stance {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Supports => "supports",
            Self::Contradicts => "contradicts",
            Self::Neutral => "neutral",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "supports" => Some(Self::Supports),
            "contradicts" => Some(Self::Contradicts),
            "neutral" => Some(Self::Neutral),
            _ => None,
        }
    }
}

impl fmt::Display for Stance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceLinkRequest {
    pub hypothesis_id: String,
    pub observation_id: Option<String>,
    pub source: Option<String>,
    pub grade: EvidenceGrade,
    pub stance: Stance,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceLink {
    pub id: String,
    pub hypothesis_id: String,
    pub observation_id: Option<String>,
    pub source: Option<String>,
    pub grade: EvidenceGrade,
    pub stance: Stance,
    pub note: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InconclusiveKind {
    Provisional { missing: Vec<String> },
    Fundamental { frontier: String },
}

impl InconclusiveKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Provisional { .. } => "provisional",
            Self::Fundamental { .. } => "fundamental",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Affirmed,
    Refuted,
    Inconclusive(InconclusiveKind),
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Affirmed => "affirmed",
            Self::Refuted => "refuted",
            Self::Inconclusive(_) => "inconclusive",
        }
    }

    pub fn to_json(&self) -> String {
        match self {
            Self::Affirmed | Self::Refuted => {
                format!("{{\"verdict\":\"{}\"}}", self.as_str())
            }
            Self::Inconclusive(InconclusiveKind::Provisional { missing }) => format!(
                concat!(
                    "{{",
                    "\"verdict\":\"inconclusive\",",
                    "\"inconclusive_kind\":\"provisional\",",
                    "\"missing\":{}",
                    "}}"
                ),
                json_string_array(missing)
            ),
            Self::Inconclusive(InconclusiveKind::Fundamental { frontier }) => format!(
                concat!(
                    "{{",
                    "\"verdict\":\"inconclusive\",",
                    "\"inconclusive_kind\":\"fundamental\",",
                    "\"frontier\":\"{}\"",
                    "}}"
                ),
                escape_json(frontier)
            ),
        }
    }

    pub fn from_json(json: &str) -> Option<Self> {
        match json_string_field(json, "verdict")?.as_str() {
            "affirmed" => Some(Self::Affirmed),
            "refuted" => Some(Self::Refuted),
            "inconclusive" => match json_string_field(json, "inconclusive_kind")?.as_str() {
                "provisional" => Some(Self::Inconclusive(InconclusiveKind::Provisional {
                    missing: json_string_array_field(json, "missing")?,
                })),
                "fundamental" => Some(Self::Inconclusive(InconclusiveKind::Fundamental {
                    frontier: json_string_field(json, "frontier")?,
                })),
                _ => None,
            },
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerdictReport {
    pub hypothesis_id: String,
    pub verdict: Verdict,
    pub confidence: Confidence,
    pub supporting: Vec<EvidenceLink>,
    pub contradicting: Vec<EvidenceLink>,
    pub rationale: String,
}

pub trait ArgumentEngine {
    fn render(&self, hypothesis_id: &str, evidence: &[EvidenceLink]) -> VerdictReport;
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RuleBasedEngine;

impl ArgumentEngine for RuleBasedEngine {
    fn render(&self, hypothesis_id: &str, evidence: &[EvidenceLink]) -> VerdictReport {
        let supporting = evidence_by_stance(evidence, Stance::Supports);
        let contradicting = evidence_by_stance(evidence, Stance::Contradicts);
        let neutral_count = evidence
            .iter()
            .filter(|link| link.stance == Stance::Neutral)
            .count();
        let support = score_for(&supporting);
        let contra = score_for(&contradicting);
        let has_obs_support = has_observed(&supporting);
        let has_obs_contra = has_observed(&contradicting);
        let stats = RuleStats {
            support,
            contra,
            total: evidence.len(),
            neutral: neutral_count,
            has_obs_support,
            has_obs_contra,
        };

        if evidence.is_empty() {
            return verdict_report(
                hypothesis_id,
                Verdict::Inconclusive(InconclusiveKind::Provisional {
                    missing: vec!["no evidence linked yet".to_string()],
                }),
                Confidence::Low,
                supporting,
                contradicting,
                rationale_for(1, stats, "no evidence linked yet"),
            );
        }

        let affirm_margin = support - contra;
        if affirm_margin >= AFFIRM_MARGIN && has_obs_support {
            let confidence = if affirm_margin >= STRONG_MARGIN && contra == 0 {
                Confidence::High
            } else {
                Confidence::Medium
            };
            return verdict_report(
                hypothesis_id,
                Verdict::Affirmed,
                confidence,
                supporting,
                contradicting,
                rationale_for(
                    2,
                    stats,
                    "support reached decision margin with observed support",
                ),
            );
        }

        let refute_margin = contra - support;
        if refute_margin >= REFUTE_MARGIN && has_obs_contra {
            let confidence = if refute_margin >= STRONG_MARGIN && support == 0 {
                Confidence::High
            } else {
                Confidence::Medium
            };
            return verdict_report(
                hypothesis_id,
                Verdict::Refuted,
                confidence,
                supporting,
                contradicting,
                rationale_for(
                    3,
                    stats,
                    "contradiction reached decision margin with observed contradiction",
                ),
            );
        }

        if support == 0 && contra == 0 {
            return verdict_report(
                hypothesis_id,
                Verdict::Inconclusive(InconclusiveKind::Provisional {
                    missing: vec![
                        "only weak/unsupported grades; need observed/inferred evidence".to_string(),
                    ],
                }),
                Confidence::Low,
                supporting,
                contradicting,
                rationale_for(4, stats, "only zero-weight evidence grades were linked"),
            );
        }

        verdict_report(
            hypothesis_id,
            Verdict::Inconclusive(InconclusiveKind::Provisional {
                missing: vec![
                    "evidence below decision margin; need stronger or more decisive evidence"
                        .to_string(),
                ],
            }),
            Confidence::Medium,
            supporting,
            contradicting,
            rationale_for(
                5,
                stats,
                "evidence had non-zero score but did not meet decision rule",
            ),
        )
    }
}

impl ProjectStore {
    pub fn link_evidence(
        &self,
        request: EvidenceLinkRequest,
    ) -> Result<EvidenceLink, StorageError> {
        validate_evidence_link_request(&request)?;
        self.inspect_hypothesis(&request.hypothesis_id)?;

        let id = self.append_event(EventRecord {
            flow_id: None,
            step_id: None,
            run_id: None,
            event_type: EVIDENCE_LINKED_EVENT.to_string(),
            payload_json: evidence_linked_payload_json(&request),
        })?;
        self.touch_project()?;
        self.evidence_for(&request.hypothesis_id)?
            .into_iter()
            .find(|link| link.id == id)
            .ok_or_else(|| StorageError::NotFound(format!("evidence link {id}")))
    }

    pub fn evidence_for(&self, hypothesis_id: &str) -> Result<Vec<EvidenceLink>, StorageError> {
        self.inspect_hypothesis(hypothesis_id)?;

        let mut stmt = self.connection().prepare(
            "SELECT id, payload_json, created_at
             FROM events
             WHERE event_type = ?1
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map(params![EVIDENCE_LINKED_EVENT], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;

        let mut evidence = Vec::new();
        for row in rows {
            let (event_id, payload_json, created_at) = row?;
            if json_string_field(&payload_json, "hypothesis_id").as_deref() == Some(hypothesis_id) {
                evidence.push(evidence_from_event(event_id, &payload_json, created_at)?);
            }
        }
        Ok(evidence)
    }

    pub fn render_verdict(
        &self,
        hypothesis_id: &str,
        engine: &dyn ArgumentEngine,
    ) -> Result<VerdictReport, StorageError> {
        let evidence = self.evidence_for(hypothesis_id)?;
        let report = engine.render(hypothesis_id, &evidence);
        self.append_event(EventRecord {
            flow_id: None,
            step_id: None,
            run_id: None,
            event_type: VERDICT_RENDERED_EVENT.to_string(),
            payload_json: verdict_rendered_payload_json(&report),
        })?;
        self.touch_project()?;
        Ok(report)
    }
}

fn evidence_by_stance(evidence: &[EvidenceLink], stance: Stance) -> Vec<EvidenceLink> {
    evidence
        .iter()
        .filter(|link| link.stance == stance)
        .cloned()
        .collect()
}

fn score_for(evidence: &[EvidenceLink]) -> i32 {
    evidence.iter().map(|link| link.grade.weight()).sum()
}

fn has_observed(evidence: &[EvidenceLink]) -> bool {
    evidence
        .iter()
        .any(|link| link.grade == EvidenceGrade::Observed)
}

fn verdict_report(
    hypothesis_id: &str,
    verdict: Verdict,
    confidence: Confidence,
    supporting: Vec<EvidenceLink>,
    contradicting: Vec<EvidenceLink>,
    rationale: String,
) -> VerdictReport {
    VerdictReport {
        hypothesis_id: hypothesis_id.to_string(),
        verdict,
        confidence,
        supporting,
        contradicting,
        rationale,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuleStats {
    support: i32,
    contra: i32,
    total: usize,
    neutral: usize,
    has_obs_support: bool,
    has_obs_contra: bool,
}

fn rationale_for(rule: usize, stats: RuleStats, detail: &str) -> String {
    format!(
        "rule {rule}: support={}; contra={}; margin={}; total={}; neutral={}; has_obs_support={}; has_obs_contra={}; {detail}",
        stats.support,
        stats.contra,
        stats.support - stats.contra,
        stats.total,
        stats.neutral,
        stats.has_obs_support,
        stats.has_obs_contra
    )
}

fn validate_evidence_link_request(request: &EvidenceLinkRequest) -> Result<(), StorageError> {
    validate_non_empty("hypothesis id", &request.hypothesis_id)?;
    validate_non_empty("evidence note", &request.note)?;
    Ok(())
}

fn validate_non_empty(label: &str, value: &str) -> Result<(), StorageError> {
    if value.trim().is_empty() {
        Err(StorageError::InvalidInput(format!(
            "argument {label} must not be empty"
        )))
    } else {
        Ok(())
    }
}

fn evidence_linked_payload_json(request: &EvidenceLinkRequest) -> String {
    format!(
        concat!(
            "{{",
            "\"hypothesis_id\":\"{}\",",
            "\"observation_id\":{},",
            "\"source\":{},",
            "\"grade\":\"{}\",",
            "\"stance\":\"{}\",",
            "\"note\":\"{}\"",
            "}}"
        ),
        escape_json(request.hypothesis_id.trim()),
        optional_json_string(request.observation_id.as_deref().map(str::trim)),
        optional_json_string(request.source.as_deref().map(str::trim)),
        request.grade.as_str(),
        request.stance.as_str(),
        escape_json(request.note.trim())
    )
}

fn evidence_from_event(
    id: String,
    payload_json: &str,
    created_at: i64,
) -> Result<EvidenceLink, StorageError> {
    Ok(EvidenceLink {
        id: id.clone(),
        hypothesis_id: required_json_string(&id, payload_json, "hypothesis_id")?,
        observation_id: json_nullable_string_field(payload_json, "observation_id"),
        source: json_nullable_string_field(payload_json, "source"),
        grade: parse_grade(&id, payload_json, "grade")?,
        stance: parse_stance(&id, payload_json, "stance")?,
        note: required_json_string(&id, payload_json, "note")?,
        created_at,
    })
}

fn verdict_rendered_payload_json(report: &VerdictReport) -> String {
    format!(
        concat!(
            "{{",
            "\"hypothesis_id\":\"{}\",",
            "\"verdict\":\"{}\",",
            "\"confidence\":\"{}\",",
            "\"rationale\":\"{}\"",
            "}}"
        ),
        escape_json(&report.hypothesis_id),
        escape_json(&verdict_payload_text(&report.verdict)),
        report.confidence.as_str(),
        escape_json(&report.rationale)
    )
}

fn verdict_payload_text(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Affirmed | Verdict::Refuted => verdict.as_str().to_string(),
        Verdict::Inconclusive(kind) => format!("inconclusive_{}", kind.as_str()),
    }
}

fn required_json_string(
    event_id: &str,
    payload_json: &str,
    field: &str,
) -> Result<String, StorageError> {
    json_string_field(payload_json, field).ok_or_else(|| {
        StorageError::InvalidInput(format!("argument event {event_id} is missing {field}"))
    })
}

fn parse_grade(
    event_id: &str,
    payload_json: &str,
    field: &str,
) -> Result<EvidenceGrade, StorageError> {
    let value = required_json_string(event_id, payload_json, field)?;
    EvidenceGrade::parse(&value).ok_or_else(|| {
        StorageError::InvalidInput(format!(
            "argument event {event_id} has invalid evidence grade {value}"
        ))
    })
}

fn parse_stance(event_id: &str, payload_json: &str, field: &str) -> Result<Stance, StorageError> {
    let value = required_json_string(event_id, payload_json, field)?;
    Stance::parse(&value).ok_or_else(|| {
        StorageError::InvalidInput(format!(
            "argument event {event_id} has invalid stance {value}"
        ))
    })
}

fn optional_json_string(value: Option<&str>) -> String {
    value.filter(|inner| !inner.trim().is_empty()).map_or_else(
        || "null".to_string(),
        |inner| format!("\"{}\"", escape_json(inner)),
    )
}

fn json_string_array(values: &[String]) -> String {
    let items = values
        .iter()
        .map(|value| format!("\"{}\"", escape_json(value)))
        .collect::<Vec<_>>()
        .join(",");
    format!("[{items}]")
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

fn json_string_array_field(json: &str, field: &str) -> Option<Vec<String>> {
    let marker = format!("\"{field}\":[");
    let start = json.find(&marker)? + marker.len();
    let rest = &json[start..];
    let end = find_json_array_end(rest)?;
    parse_json_string_array(&rest[..end])
}

fn parse_json_string_array(input: &str) -> Option<Vec<String>> {
    let mut values = Vec::new();
    let mut rest = input.trim();
    if rest.is_empty() {
        return Some(values);
    }

    loop {
        rest = rest.trim_start();
        let string_body = rest.strip_prefix('"')?;
        let end = find_json_string_end(string_body)?;
        values.push(unescape_json_string(&string_body[..end]));
        rest = string_body[end + 1..].trim_start();
        if rest.is_empty() {
            return Some(values);
        }
        rest = rest.strip_prefix(',')?;
    }
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

fn find_json_array_end(input: &str) -> Option<usize> {
    let mut escaped = false;
    let mut in_string = false;
    for (index, ch) in input.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            ']' if !in_string => return Some(index),
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

    use crate::hypothesis::{HypothesisRequest, HypothesisStatus};
    use crate::storage::{now_unix_seconds, ProjectStore};

    use super::{
        ArgumentEngine, EvidenceGrade, EvidenceLink, EvidenceLinkRequest, InconclusiveKind,
        RuleBasedEngine, Stance, Verdict,
    };

    fn temp_project_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentflow-argument-{test_name}-{}-{}",
            std::process::id(),
            now_unix_seconds()
        ))
    }

    fn record_hypothesis(store: &ProjectStore) -> String {
        store
            .record_hypothesis(HypothesisRequest {
                statement: "Marker A supports pathway B".to_string(),
                origin: "agent".to_string(),
                related_goal_id: "goal_argument".to_string(),
            })
            .unwrap()
            .id
    }

    fn evidence_link(id: &str, grade: EvidenceGrade, stance: Stance) -> EvidenceLink {
        EvidenceLink {
            id: id.to_string(),
            hypothesis_id: "hypothesis_1".to_string(),
            observation_id: None,
            source: None,
            grade,
            stance,
            note: format!("{stance} via {grade}"),
            created_at: 1,
        }
    }

    #[test]
    fn links_and_lists_evidence_for_hypothesis() {
        let path = temp_project_path("link");
        let store = ProjectStore::init(&path, Some("Argument Demo")).unwrap();
        let hypothesis_id = record_hypothesis(&store);
        let other_hypothesis_id = store
            .record_hypothesis(HypothesisRequest {
                statement: "Other hypothesis".to_string(),
                origin: "agent".to_string(),
                related_goal_id: "goal_other".to_string(),
            })
            .unwrap()
            .id;

        let evidence = store
            .link_evidence(EvidenceLinkRequest {
                hypothesis_id: hypothesis_id.clone(),
                observation_id: Some(" observation_1 ".to_string()),
                source: Some(" literature note ".to_string()),
                grade: EvidenceGrade::Observed,
                stance: Stance::Supports,
                note: " Observed marker increased ".to_string(),
            })
            .unwrap();
        store
            .link_evidence(EvidenceLinkRequest {
                hypothesis_id: other_hypothesis_id,
                observation_id: None,
                source: None,
                grade: EvidenceGrade::Inferred,
                stance: Stance::Contradicts,
                note: "Other ledger evidence".to_string(),
            })
            .unwrap();

        assert!(evidence.id.starts_with("event_"));
        assert_eq!(evidence.hypothesis_id, hypothesis_id);
        assert_eq!(evidence.observation_id.as_deref(), Some("observation_1"));
        assert_eq!(evidence.source.as_deref(), Some("literature note"));
        assert_eq!(evidence.note, "Observed marker increased");

        let evidence = store.evidence_for(&hypothesis_id).unwrap();
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].grade, EvidenceGrade::Observed);
        assert_eq!(evidence[0].stance, Stance::Supports);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn link_evidence_rejects_missing_hypothesis_and_empty_note() {
        let path = temp_project_path("reject-link");
        let store = ProjectStore::init(&path, Some("Argument Demo")).unwrap();
        let error = store
            .link_evidence(EvidenceLinkRequest {
                hypothesis_id: "missing_hypothesis".to_string(),
                observation_id: None,
                source: None,
                grade: EvidenceGrade::Observed,
                stance: Stance::Supports,
                note: "Evidence".to_string(),
            })
            .unwrap_err();
        assert!(error.to_string().contains("not found"));

        let hypothesis_id = record_hypothesis(&store);
        let error = store
            .link_evidence(EvidenceLinkRequest {
                hypothesis_id,
                observation_id: None,
                source: None,
                grade: EvidenceGrade::Observed,
                stance: Stance::Supports,
                note: " ".to_string(),
            })
            .unwrap_err();
        assert!(error.to_string().contains("argument evidence note"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn rule_engine_affirms_with_observed_margin() {
        let engine = RuleBasedEngine;
        let evidence = vec![
            evidence_link("e1", EvidenceGrade::Observed, Stance::Supports),
            evidence_link("e2", EvidenceGrade::Observed, Stance::Supports),
        ];

        let report = engine.render("hypothesis_1", &evidence);

        assert_eq!(report.verdict, Verdict::Affirmed);
        assert_eq!(report.confidence, crate::hypothesis::Confidence::High);
        assert_eq!(report.supporting.len(), 2);
        assert_eq!(report.contradicting.len(), 0);
        assert!(report.rationale.contains("rule 2"));
        assert!(report.rationale.contains("support=6"));
    }

    #[test]
    fn rule_engine_refutes_with_observed_margin() {
        let engine = RuleBasedEngine;
        let evidence = vec![evidence_link(
            "e1",
            EvidenceGrade::Observed,
            Stance::Contradicts,
        )];

        let report = engine.render("hypothesis_1", &evidence);

        assert_eq!(report.verdict, Verdict::Refuted);
        assert_eq!(report.confidence, crate::hypothesis::Confidence::Medium);
        assert_eq!(report.supporting.len(), 0);
        assert_eq!(report.contradicting.len(), 1);
        assert!(report.rationale.contains("rule 3"));
        assert!(report.rationale.contains("contra=3"));
    }

    #[test]
    fn rule_engine_is_inconclusive_with_no_evidence() {
        let engine = RuleBasedEngine;
        let report = engine.render("hypothesis_1", &[]);

        assert_eq!(
            report.verdict,
            Verdict::Inconclusive(InconclusiveKind::Provisional {
                missing: vec!["no evidence linked yet".to_string()]
            })
        );
        assert_eq!(report.confidence, crate::hypothesis::Confidence::Low);
        assert!(report.rationale.contains("rule 1"));
    }

    #[test]
    fn rule_engine_is_inconclusive_with_only_zero_weight_evidence() {
        let engine = RuleBasedEngine;
        let evidence = vec![
            evidence_link("e1", EvidenceGrade::Hypothesis, Stance::Supports),
            evidence_link("e2", EvidenceGrade::Unsupported, Stance::Contradicts),
            evidence_link("e3", EvidenceGrade::Unsupported, Stance::Neutral),
        ];

        let report = engine.render("hypothesis_1", &evidence);

        assert_eq!(
            report.verdict,
            Verdict::Inconclusive(InconclusiveKind::Provisional {
                missing: vec![
                    "only weak/unsupported grades; need observed/inferred evidence".to_string()
                ]
            })
        );
        assert_eq!(report.confidence, crate::hypothesis::Confidence::Low);
        assert_eq!(report.supporting.len(), 1);
        assert_eq!(report.contradicting.len(), 1);
        assert!(report.rationale.contains("rule 4"));
    }

    #[test]
    fn rule_engine_is_inconclusive_below_margin() {
        let engine = RuleBasedEngine;
        let evidence = vec![evidence_link(
            "e1",
            EvidenceGrade::Inferred,
            Stance::Supports,
        )];

        let report = engine.render("hypothesis_1", &evidence);

        assert_eq!(
            report.verdict,
            Verdict::Inconclusive(InconclusiveKind::Provisional {
                missing: vec![
                    "evidence below decision margin; need stronger or more decisive evidence"
                        .to_string()
                ]
            })
        );
        assert_eq!(report.confidence, crate::hypothesis::Confidence::Medium);
        assert!(report.rationale.contains("rule 5"));
        assert!(report.rationale.contains("support=2"));
    }

    #[test]
    fn render_verdict_records_event_without_transitioning_hypothesis() {
        let path = temp_project_path("render-verdict");
        let store = ProjectStore::init(&path, Some("Argument Demo")).unwrap();
        let hypothesis_id = record_hypothesis(&store);
        store
            .link_evidence(EvidenceLinkRequest {
                hypothesis_id: hypothesis_id.clone(),
                observation_id: Some("observation_2".to_string()),
                source: None,
                grade: EvidenceGrade::Observed,
                stance: Stance::Supports,
                note: "Observed support".to_string(),
            })
            .unwrap();

        let report = store
            .render_verdict(&hypothesis_id, &RuleBasedEngine)
            .unwrap();

        assert_eq!(report.verdict, Verdict::Affirmed);
        assert_eq!(
            store.inspect_hypothesis(&hypothesis_id).unwrap().status,
            HypothesisStatus::Proposed
        );
        let count: i64 = store
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM events WHERE event_type = 'argument.verdict_rendered'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn inconclusive_verdict_json_round_trips_provisional_and_fundamental() {
        let provisional = Verdict::Inconclusive(InconclusiveKind::Provisional {
            missing: vec![
                "need observed evidence".to_string(),
                "need decisive falsifier".to_string(),
            ],
        });
        assert_eq!(
            Verdict::from_json(&provisional.to_json()).unwrap(),
            provisional
        );

        let fundamental = Verdict::Inconclusive(InconclusiveKind::Fundamental {
            frontier: "requires external replication".to_string(),
        });
        assert_eq!(
            Verdict::from_json(&fundamental.to_json()).unwrap(),
            fundamental
        );
    }
}
