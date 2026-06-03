use std::fmt;

use rusqlite::params;

use crate::domain::ToolMaturity;
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

impl EvidenceLink {
    pub fn to_json(&self) -> String {
        format!(
            concat!(
                "{{",
                "\"id\":\"{}\",",
                "\"hypothesis_id\":\"{}\",",
                "\"observation_id\":{},",
                "\"source\":{},",
                "\"grade\":\"{}\",",
                "\"stance\":\"{}\",",
                "\"note\":\"{}\",",
                "\"created_at\":{}",
                "}}"
            ),
            escape_json(&self.id),
            escape_json(&self.hypothesis_id),
            optional_json_string(self.observation_id.as_deref()),
            optional_json_string(self.source.as_deref()),
            self.grade.as_str(),
            self.stance.as_str(),
            escape_json(&self.note),
            self.created_at
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClaimBasis {
    Observed,
    StatisticallyInferred,
    Speculative,
}

impl ClaimBasis {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::StatisticallyInferred => "statistically_inferred",
            Self::Speculative => "speculative",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "observed" => Some(Self::Observed),
            "statistically_inferred" => Some(Self::StatisticallyInferred),
            "speculative" => Some(Self::Speculative),
            _ => None,
        }
    }
}

impl fmt::Display for ClaimBasis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfDeceptionGate {
    pub supports: String,
    pub against: String,
    pub alternatives: String,
    pub data_quality_risks: String,
    pub assumptions: String,
    pub falsifier: String,
    pub claim_basis: ClaimBasis,
    pub not_yet_claimable: String,
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

impl VerdictReport {
    pub fn to_json(&self) -> String {
        let (inconclusive_kind, missing, frontier) = match &self.verdict {
            Verdict::Inconclusive(InconclusiveKind::Provisional { missing }) => {
                (Some("provisional"), json_string_array(missing), None)
            }
            Verdict::Inconclusive(InconclusiveKind::Fundamental { frontier }) => (
                Some("fundamental"),
                "[]".to_string(),
                Some(frontier.as_str()),
            ),
            Verdict::Affirmed | Verdict::Refuted => (None, "[]".to_string(), None),
        };
        let supporting = self
            .supporting
            .iter()
            .map(EvidenceLink::to_json)
            .collect::<Vec<_>>()
            .join(",");
        let contradicting = self
            .contradicting
            .iter()
            .map(EvidenceLink::to_json)
            .collect::<Vec<_>>()
            .join(",");

        format!(
            concat!(
                "{{",
                "\"hypothesis_id\":\"{}\",",
                "\"verdict\":\"{}\",",
                "\"inconclusive_kind\":{},",
                "\"missing\":{},",
                "\"frontier\":{},",
                "\"confidence\":\"{}\",",
                "\"rationale\":\"{}\",",
                "\"supporting\":[{}],",
                "\"contradicting\":[{}]",
                "}}"
            ),
            escape_json(&self.hypothesis_id),
            self.verdict.as_str(),
            optional_json_string(inconclusive_kind),
            missing,
            optional_json_string(frontier),
            self.confidence.as_str(),
            escape_json(&self.rationale),
            supporting,
            contradicting
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VerdictTag {
    Affirmed,
    Refuted,
    InconclusiveProvisional,
    InconclusiveFundamental,
}

impl VerdictTag {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Affirmed => "affirmed",
            Self::Refuted => "refuted",
            Self::InconclusiveProvisional => "inconclusive_provisional",
            Self::InconclusiveFundamental => "inconclusive_fundamental",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "affirmed" => Some(Self::Affirmed),
            "refuted" => Some(Self::Refuted),
            "inconclusive_provisional" => Some(Self::InconclusiveProvisional),
            "inconclusive_fundamental" => Some(Self::InconclusiveFundamental),
            _ => None,
        }
    }
}

impl fmt::Display for VerdictTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerdictSummary {
    pub hypothesis_id: String,
    pub tag: VerdictTag,
    pub confidence: Confidence,
    pub frontier: Option<String>,
    pub created_at: i64,
}

impl VerdictSummary {
    pub fn to_json(&self) -> String {
        format!(
            concat!(
                "{{",
                "\"hypothesis_id\":\"{}\",",
                "\"tag\":\"{}\",",
                "\"confidence\":\"{}\",",
                "\"frontier\":{},",
                "\"created_at\":{}",
                "}}"
            ),
            escape_json(&self.hypothesis_id),
            self.tag.as_str(),
            self.confidence.as_str(),
            optional_json_string(self.frontier.as_deref()),
            self.created_at
        )
    }
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
        mut request: EvidenceLinkRequest,
    ) -> Result<EvidenceLink, StorageError> {
        validate_evidence_link_request(&request)?;
        self.inspect_hypothesis(&request.hypothesis_id)?;
        request.grade = self.capped_evidence_grade(&request)?;

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

    fn capped_evidence_grade(
        &self,
        request: &EvidenceLinkRequest,
    ) -> Result<EvidenceGrade, StorageError> {
        if request.grade != EvidenceGrade::Observed {
            return Ok(request.grade);
        }

        let Some(observation_id) = request.observation_id.as_deref() else {
            return Ok(request.grade);
        };
        if self.source_tool_maturity_for_observation(observation_id)?
            == Some(ToolMaturity::Exploratory)
        {
            return Ok(EvidenceGrade::Inferred);
        }

        Ok(request.grade)
    }

    fn source_tool_maturity_for_observation(
        &self,
        observation_id: &str,
    ) -> Result<Option<ToolMaturity>, StorageError> {
        let observation_id = observation_id.trim();
        if observation_id.is_empty() {
            return Ok(None);
        }

        let observation = match self.inspect_observation(observation_id) {
            Ok(observation) => observation,
            Err(StorageError::NotFound(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        let (Some(flow_id), Some(step_id)) = (observation.flow_id, observation.step_id) else {
            return Ok(None);
        };

        let flow = match self.inspect_flow(&flow_id) {
            Ok(flow) => flow,
            Err(StorageError::NotFound(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        let Some(tool_ref) = flow
            .steps
            .iter()
            .find(|step| step.id == step_id || step.local_id == step_id)
            .and_then(|step| step.tool_ref.as_deref())
            .map(str::trim)
            .filter(|tool_ref| !tool_ref.is_empty())
        else {
            return Ok(None);
        };

        let inspection = match self.inspect_tool(tool_ref) {
            Ok(inspection) => inspection,
            Err(StorageError::NotFound(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(ToolMaturity::parse(&inspection.summary.maturity))
    }

    pub fn evidence_for(&self, hypothesis_id: &str) -> Result<Vec<EvidenceLink>, StorageError> {
        validate_non_empty("hypothesis id", hypothesis_id)?;
        let hypothesis_id = hypothesis_id.trim();
        let reverted = self.reverted_event_id_set()?;
        if let Err(error) = self.inspect_hypothesis(hypothesis_id) {
            if matches!(&error, StorageError::NotFound(_)) && reverted.contains(hypothesis_id) {
                return Ok(Vec::new());
            }
            return Err(error);
        }

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
            if reverted.contains(&event_id) {
                continue;
            }
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
        gate: Option<SelfDeceptionGate>,
    ) -> Result<VerdictReport, StorageError> {
        self.inspect_hypothesis(hypothesis_id)?;
        let evidence = self.evidence_for(hypothesis_id)?;
        let report = engine.render(hypothesis_id, &evidence);
        validate_self_deception_gate(&report.verdict, gate.as_ref())?;
        self.append_event(EventRecord {
            flow_id: None,
            step_id: None,
            run_id: None,
            event_type: VERDICT_RENDERED_EVENT.to_string(),
            payload_json: verdict_rendered_payload_json(&report, gate.as_ref()),
        })?;
        self.touch_project()?;
        Ok(report)
    }

    pub fn latest_verdict_for(
        &self,
        hypothesis_id: &str,
    ) -> Result<Option<VerdictSummary>, StorageError> {
        validate_non_empty("hypothesis id", hypothesis_id)?;
        let hypothesis_id = hypothesis_id.trim();
        let reverted = self.reverted_event_id_set()?;
        if let Err(error) = self.inspect_hypothesis(hypothesis_id) {
            if matches!(&error, StorageError::NotFound(_)) && reverted.contains(hypothesis_id) {
                return Ok(None);
            }
            return Err(error);
        }

        let mut stmt = self.connection().prepare(
            "SELECT id, payload_json, created_at
             FROM events
             WHERE event_type = ?1
             ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map(params![VERDICT_RENDERED_EVENT], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;

        for row in rows {
            let (event_id, payload_json, created_at) = row?;
            if reverted.contains(&event_id) {
                continue;
            }
            if json_string_field(&payload_json, "hypothesis_id").as_deref() == Some(hypothesis_id) {
                return Ok(Some(verdict_summary_from_event(
                    event_id,
                    &payload_json,
                    created_at,
                )?));
            }
        }

        Ok(None)
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

fn validate_self_deception_gate(
    verdict: &Verdict,
    gate: Option<&SelfDeceptionGate>,
) -> Result<(), StorageError> {
    if !requires_self_deception_gate(verdict) {
        return Ok(());
    }

    let gate = gate.ok_or_else(|| {
        StorageError::InvalidInput("strong verdict requires self-deception gate".to_string())
    })?;
    if gate.against.trim().is_empty() || gate.alternatives.trim().is_empty() {
        return Err(StorageError::InvalidInput(
            "self-deception gate requires against and alternatives".to_string(),
        ));
    }
    if matches!(verdict, Verdict::Affirmed) && gate.claim_basis == ClaimBasis::Speculative {
        return Err(StorageError::InvalidInput(
            "speculative basis cannot affirm".to_string(),
        ));
    }

    Ok(())
}

fn requires_self_deception_gate(verdict: &Verdict) -> bool {
    matches!(
        verdict,
        Verdict::Affirmed
            | Verdict::Refuted
            | Verdict::Inconclusive(InconclusiveKind::Fundamental { .. })
    )
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

fn verdict_rendered_payload_json(
    report: &VerdictReport,
    gate: Option<&SelfDeceptionGate>,
) -> String {
    format!(
        concat!(
            "{{",
            "\"hypothesis_id\":\"{}\",",
            "\"verdict\":\"{}\",",
            "\"confidence\":\"{}\",",
            "\"frontier\":{},",
            "\"rationale\":\"{}\",",
            "\"gate\":{}",
            "}}"
        ),
        escape_json(&report.hypothesis_id),
        escape_json(&verdict_payload_text(&report.verdict)),
        report.confidence.as_str(),
        optional_json_string(verdict_frontier(&report.verdict)),
        escape_json(&report.rationale),
        self_deception_gate_json(gate)
    )
}

fn verdict_payload_text(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Affirmed | Verdict::Refuted => verdict.as_str().to_string(),
        Verdict::Inconclusive(kind) => format!("inconclusive_{}", kind.as_str()),
    }
}

fn verdict_frontier(verdict: &Verdict) -> Option<&str> {
    match verdict {
        Verdict::Inconclusive(InconclusiveKind::Fundamental { frontier }) => Some(frontier),
        Verdict::Affirmed | Verdict::Refuted | Verdict::Inconclusive(_) => None,
    }
}

fn self_deception_gate_json(gate: Option<&SelfDeceptionGate>) -> String {
    gate.map_or_else(
        || "null".to_string(),
        |gate| {
            format!(
                concat!(
                    "{{",
                    "\"supports\":\"{}\",",
                    "\"against\":\"{}\",",
                    "\"alternatives\":\"{}\",",
                    "\"data_quality_risks\":\"{}\",",
                    "\"assumptions\":\"{}\",",
                    "\"falsifier\":\"{}\",",
                    "\"claim_basis\":\"{}\",",
                    "\"not_yet_claimable\":\"{}\"",
                    "}}"
                ),
                escape_json(gate.supports.trim()),
                escape_json(gate.against.trim()),
                escape_json(gate.alternatives.trim()),
                escape_json(gate.data_quality_risks.trim()),
                escape_json(gate.assumptions.trim()),
                escape_json(gate.falsifier.trim()),
                gate.claim_basis.as_str(),
                escape_json(gate.not_yet_claimable.trim())
            )
        },
    )
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

fn verdict_summary_from_event(
    event_id: String,
    payload_json: &str,
    created_at: i64,
) -> Result<VerdictSummary, StorageError> {
    let hypothesis_id = required_json_string(&event_id, payload_json, "hypothesis_id")?;
    let verdict = required_json_string(&event_id, payload_json, "verdict")?;
    let confidence = required_json_string(&event_id, payload_json, "confidence")?;
    let tag = VerdictTag::parse(&verdict).ok_or_else(|| {
        StorageError::InvalidInput(format!(
            "argument event {event_id} has invalid verdict {verdict}"
        ))
    })?;
    let confidence = Confidence::parse(&confidence).ok_or_else(|| {
        StorageError::InvalidInput(format!(
            "argument event {event_id} has invalid confidence {confidence}"
        ))
    })?;

    Ok(VerdictSummary {
        hypothesis_id,
        tag,
        confidence,
        frontier: json_nullable_string_field(payload_json, "frontier"),
        created_at,
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
    use std::fs;
    use std::path::PathBuf;

    use crate::hypothesis::{Confidence, HypothesisRequest, HypothesisStatus};
    use crate::storage::{now_unix_seconds, FlowDraft, ProjectStore, StorageError, ToolSpec};

    use super::{
        ArgumentEngine, ClaimBasis, EvidenceGrade, EvidenceLink, EvidenceLinkRequest,
        InconclusiveKind, RuleBasedEngine, SelfDeceptionGate, Stance, Verdict, VerdictReport,
        VerdictTag,
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

    fn valid_gate() -> SelfDeceptionGate {
        gate_with_basis(ClaimBasis::Observed)
    }

    fn gate_with_basis(claim_basis: ClaimBasis) -> SelfDeceptionGate {
        SelfDeceptionGate {
            supports: "Observed evidence supports the claim".to_string(),
            against: "Contradictory evidence has been checked".to_string(),
            alternatives: "Alternative explanations remain less consistent".to_string(),
            data_quality_risks: "Sampling bias is limited by replication".to_string(),
            assumptions: "Measurements are comparable across runs".to_string(),
            falsifier: "A replicated contradiction would overturn this claim".to_string(),
            claim_basis,
            not_yet_claimable: "No causal mechanism is claimed yet".to_string(),
        }
    }

    fn rendered_event_count(store: &ProjectStore) -> i64 {
        store
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM events WHERE event_type = 'argument.verdict_rendered'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn tool_observation(
        store: &ProjectStore,
        root: &std::path::Path,
        flow_id: &str,
        tool_name: &str,
        maturity: &str,
    ) -> String {
        let script_path = root.join(format!("{tool_name}.sh"));
        fs::write(
            &script_path,
            r#"printf '# Source evidence
score: 0.9
' > "$AGENTFLOW_OUTPUT_REPORT"
"#,
        )
        .unwrap();
        store
            .register_tool(
                ToolSpec::from_simple_yaml(&format!(
                    r#"
schema_version: agentflow.tool.v0
namespace: evidence
name: {tool_name}
version: 0.1.0
maturity: {maturity}
description: Produce argument evidence
outputs:
  report:
    type: Markdown
    observer: artifact_summary
runtime:
  backend: local
  command:
    - /bin/sh
    - {}
"#,
                    script_path.display()
                ))
                .unwrap(),
            )
            .unwrap();
        store
            .approve_flow(
                FlowDraft::from_simple_yaml(&format!(
                    r#"
schema_version: agentflow.flow.v0
id: {flow_id}
name: Evidence source
steps:
  - id: produce
    tool: evidence/{tool_name}
    reason: Produce evidence observation
    needs: []
    outputs:
      report: report
"#
                ))
                .unwrap(),
                None,
            )
            .unwrap();

        let before = store.list_observations().unwrap().len();
        let summary = store.run_flow(flow_id).unwrap();
        assert_eq!(summary.completed_steps, 1);
        assert_eq!(summary.failed_steps, 0);

        let observations = store.list_observations().unwrap();
        assert_eq!(observations.len(), before + 1);
        observations.last().unwrap().id.clone()
    }

    #[derive(Debug, Clone)]
    struct FixedEngine {
        verdict: Verdict,
        confidence: Confidence,
    }

    impl ArgumentEngine for FixedEngine {
        fn render(&self, hypothesis_id: &str, _evidence: &[EvidenceLink]) -> VerdictReport {
            VerdictReport {
                hypothesis_id: hypothesis_id.to_string(),
                verdict: self.verdict.clone(),
                confidence: self.confidence,
                supporting: Vec::new(),
                contradicting: Vec::new(),
                rationale: "fixed test verdict".to_string(),
            }
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
    fn observed_grade_is_capped_only_for_exploratory_tool_observations() {
        let path = temp_project_path("grade-cap");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Argument Demo")).unwrap();
        let exploratory_observation = tool_observation(
            &store,
            &path,
            "exploratory_flow",
            "synth_like",
            "exploratory",
        );
        let verified_observation =
            tool_observation(&store, &path, "verified_flow", "verified_tool", "verified");
        let exploratory_id = record_hypothesis(&store);
        let verified_id = record_hypothesis(&store);
        let manual_id = record_hypothesis(&store);
        let non_observed_id = record_hypothesis(&store);

        let exploratory = store
            .link_evidence(EvidenceLinkRequest {
                hypothesis_id: exploratory_id.clone(),
                observation_id: Some(exploratory_observation),
                source: None,
                grade: EvidenceGrade::Observed,
                stance: Stance::Supports,
                note: "Exploratory tool support".to_string(),
            })
            .unwrap();
        let verified = store
            .link_evidence(EvidenceLinkRequest {
                hypothesis_id: verified_id.clone(),
                observation_id: Some(verified_observation),
                source: None,
                grade: EvidenceGrade::Observed,
                stance: Stance::Supports,
                note: "Verified tool support".to_string(),
            })
            .unwrap();
        let manual = store
            .link_evidence(EvidenceLinkRequest {
                hypothesis_id: manual_id.clone(),
                observation_id: None,
                source: Some("manual note".to_string()),
                grade: EvidenceGrade::Observed,
                stance: Stance::Supports,
                note: "Manual support".to_string(),
            })
            .unwrap();
        let non_observed = store
            .link_evidence(EvidenceLinkRequest {
                hypothesis_id: non_observed_id.clone(),
                observation_id: exploratory.observation_id.clone(),
                source: None,
                grade: EvidenceGrade::LiteratureSupported,
                stance: Stance::Supports,
                note: "Exploratory source with non-observed grade".to_string(),
            })
            .unwrap();

        assert_eq!(exploratory.grade, EvidenceGrade::Inferred);
        assert_eq!(
            store.evidence_for(&exploratory_id).unwrap()[0].grade,
            EvidenceGrade::Inferred
        );
        assert_eq!(verified.grade, EvidenceGrade::Observed);
        assert_eq!(
            store.evidence_for(&verified_id).unwrap()[0].grade,
            EvidenceGrade::Observed
        );
        assert_eq!(manual.grade, EvidenceGrade::Observed);
        assert_eq!(
            store.evidence_for(&manual_id).unwrap()[0].grade,
            EvidenceGrade::Observed
        );
        assert_eq!(non_observed.grade, EvidenceGrade::LiteratureSupported);
        assert_eq!(
            store.evidence_for(&non_observed_id).unwrap()[0].grade,
            EvidenceGrade::LiteratureSupported
        );

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn exploratory_tool_evidence_cannot_independently_affirm_but_verified_can() {
        let path = temp_project_path("grade-cap-verdict");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Argument Demo")).unwrap();
        let exploratory_observation = tool_observation(
            &store,
            &path,
            "exploratory_verdict",
            "synth_verdict",
            "exploratory",
        );
        let verified_observation = tool_observation(
            &store,
            &path,
            "verified_verdict",
            "verified_verdict_tool",
            "verified",
        );
        let exploratory_id = record_hypothesis(&store);
        let verified_id = record_hypothesis(&store);

        store
            .link_evidence(EvidenceLinkRequest {
                hypothesis_id: exploratory_id.clone(),
                observation_id: Some(exploratory_observation),
                source: None,
                grade: EvidenceGrade::Observed,
                stance: Stance::Supports,
                note: "Exploratory support".to_string(),
            })
            .unwrap();
        store
            .link_evidence(EvidenceLinkRequest {
                hypothesis_id: verified_id.clone(),
                observation_id: Some(verified_observation),
                source: None,
                grade: EvidenceGrade::Observed,
                stance: Stance::Supports,
                note: "Verified support".to_string(),
            })
            .unwrap();

        let exploratory_report = store
            .render_verdict(&exploratory_id, &RuleBasedEngine, None)
            .unwrap();
        let verified_report = store
            .render_verdict(&verified_id, &RuleBasedEngine, Some(valid_gate()))
            .unwrap();

        assert!(matches!(
            exploratory_report.verdict,
            Verdict::Inconclusive(InconclusiveKind::Provisional { .. })
        ));
        assert!(exploratory_report
            .rationale
            .contains("has_obs_support=false"));
        assert_eq!(verified_report.verdict, Verdict::Affirmed);

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn claim_basis_round_trips_payload_text() {
        for basis in [
            ClaimBasis::Observed,
            ClaimBasis::StatisticallyInferred,
            ClaimBasis::Speculative,
        ] {
            assert_eq!(ClaimBasis::parse(basis.as_str()), Some(basis));
            assert_eq!(basis.to_string(), basis.as_str());
        }
        assert_eq!(ClaimBasis::parse("inferred"), None);
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
            .render_verdict(&hypothesis_id, &RuleBasedEngine, Some(valid_gate()))
            .unwrap();

        assert_eq!(report.verdict, Verdict::Affirmed);
        assert_eq!(
            store.inspect_hypothesis(&hypothesis_id).unwrap().status,
            HypothesisStatus::Proposed
        );
        assert_eq!(rendered_event_count(&store), 1);
        let payload: String = store
            .connection()
            .query_row(
                "SELECT payload_json FROM events WHERE event_type = 'argument.verdict_rendered'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(payload.contains("\"gate\":{"));
        assert!(payload.contains("\"claim_basis\":\"observed\""));
        assert!(payload.contains("\"against\":\"Contradictory evidence has been checked\""));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn strong_verdicts_accept_valid_self_deception_gate() {
        let path = temp_project_path("valid-gates");
        let store = ProjectStore::init(&path, Some("Argument Demo")).unwrap();
        let cases = vec![
            ("Affirmed gate", Verdict::Affirmed, Confidence::High),
            ("Refuted gate", Verdict::Refuted, Confidence::Medium),
            (
                "Fundamental gate",
                Verdict::Inconclusive(InconclusiveKind::Fundamental {
                    frontier: "requires external replication".to_string(),
                }),
                Confidence::Low,
            ),
        ];

        for (statement, verdict, confidence) in cases {
            let hypothesis_id = store
                .record_hypothesis(HypothesisRequest {
                    statement: statement.to_string(),
                    origin: "agent".to_string(),
                    related_goal_id: "goal_argument".to_string(),
                })
                .unwrap()
                .id;
            let report = store
                .render_verdict(
                    &hypothesis_id,
                    &FixedEngine {
                        verdict: verdict.clone(),
                        confidence,
                    },
                    Some(valid_gate()),
                )
                .unwrap();

            assert_eq!(report.verdict, verdict);
        }
        assert_eq!(rendered_event_count(&store), 3);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn strong_verdict_rejects_missing_self_deception_gate() {
        let path = temp_project_path("missing-gate");
        let store = ProjectStore::init(&path, Some("Argument Demo")).unwrap();
        let hypothesis_id = record_hypothesis(&store);

        let error = store
            .render_verdict(
                &hypothesis_id,
                &FixedEngine {
                    verdict: Verdict::Affirmed,
                    confidence: Confidence::High,
                },
                None,
            )
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "strong verdict requires self-deception gate"
        );
        assert_eq!(rendered_event_count(&store), 0);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn strong_verdict_rejects_empty_against_or_alternatives() {
        let path = temp_project_path("empty-gate-core");
        let store = ProjectStore::init(&path, Some("Argument Demo")).unwrap();
        let refuted_id = record_hypothesis(&store);
        let fundamental_id = store
            .record_hypothesis(HypothesisRequest {
                statement: "Fundamental needs gate".to_string(),
                origin: "agent".to_string(),
                related_goal_id: "goal_argument".to_string(),
            })
            .unwrap()
            .id;
        let mut missing_against = valid_gate();
        missing_against.against = " ".to_string();
        let mut missing_alternatives = valid_gate();
        missing_alternatives.alternatives = " ".to_string();

        let error = store
            .render_verdict(
                &refuted_id,
                &FixedEngine {
                    verdict: Verdict::Refuted,
                    confidence: Confidence::Medium,
                },
                Some(missing_against),
            )
            .unwrap_err();
        assert!(error.to_string().contains("against and alternatives"));

        let error = store
            .render_verdict(
                &fundamental_id,
                &FixedEngine {
                    verdict: Verdict::Inconclusive(InconclusiveKind::Fundamental {
                        frontier: "external replication".to_string(),
                    }),
                    confidence: Confidence::Low,
                },
                Some(missing_alternatives),
            )
            .unwrap_err();
        assert!(error.to_string().contains("against and alternatives"));
        assert_eq!(rendered_event_count(&store), 0);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn affirmed_verdict_rejects_speculative_gate() {
        let path = temp_project_path("speculative-affirm");
        let store = ProjectStore::init(&path, Some("Argument Demo")).unwrap();
        let hypothesis_id = record_hypothesis(&store);

        let error = store
            .render_verdict(
                &hypothesis_id,
                &FixedEngine {
                    verdict: Verdict::Affirmed,
                    confidence: Confidence::High,
                },
                Some(gate_with_basis(ClaimBasis::Speculative)),
            )
            .unwrap_err();

        assert_eq!(error.to_string(), "speculative basis cannot affirm");
        assert_eq!(rendered_event_count(&store), 0);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn provisional_verdict_accepts_missing_or_optional_gate() {
        let path = temp_project_path("provisional-gate");
        let store = ProjectStore::init(&path, Some("Argument Demo")).unwrap();
        let first_id = record_hypothesis(&store);
        let second_id = store
            .record_hypothesis(HypothesisRequest {
                statement: "Optional gate is stored".to_string(),
                origin: "agent".to_string(),
                related_goal_id: "goal_argument".to_string(),
            })
            .unwrap()
            .id;
        let provisional = Verdict::Inconclusive(InconclusiveKind::Provisional {
            missing: vec!["more evidence".to_string()],
        });
        let mut optional_gate = gate_with_basis(ClaimBasis::Speculative);
        optional_gate.against.clear();
        optional_gate.alternatives.clear();

        store
            .render_verdict(
                &first_id,
                &FixedEngine {
                    verdict: provisional.clone(),
                    confidence: Confidence::Low,
                },
                None,
            )
            .unwrap();
        store
            .render_verdict(
                &second_id,
                &FixedEngine {
                    verdict: provisional,
                    confidence: Confidence::Low,
                },
                Some(optional_gate),
            )
            .unwrap();

        let mut stmt = store
            .connection()
            .prepare(
                "SELECT payload_json FROM events
                 WHERE event_type = 'argument.verdict_rendered'
                 ORDER BY created_at ASC, id ASC",
            )
            .unwrap();
        let payloads = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(payloads.len(), 2);
        assert!(payloads[0].contains("\"gate\":null"));
        assert!(payloads[1].contains("\"claim_basis\":\"speculative\""));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn verdict_tag_round_trips_payload_text() {
        for tag in [
            VerdictTag::Affirmed,
            VerdictTag::Refuted,
            VerdictTag::InconclusiveProvisional,
            VerdictTag::InconclusiveFundamental,
        ] {
            assert_eq!(VerdictTag::parse(tag.as_str()), Some(tag));
            assert_eq!(tag.to_string(), tag.as_str());
        }
        assert_eq!(VerdictTag::parse("inconclusive"), None);
    }

    #[test]
    fn latest_verdict_for_returns_newest_summary_or_none() {
        let path = temp_project_path("latest-verdict");
        let store = ProjectStore::init(&path, Some("Argument Demo")).unwrap();
        let hypothesis_id = record_hypothesis(&store);
        let empty_hypothesis_id = store
            .record_hypothesis(HypothesisRequest {
                statement: "No verdict yet".to_string(),
                origin: "agent".to_string(),
                related_goal_id: "goal_argument".to_string(),
            })
            .unwrap()
            .id;

        assert!(store
            .latest_verdict_for(&empty_hypothesis_id)
            .unwrap()
            .is_none());

        store
            .render_verdict(
                &hypothesis_id,
                &FixedEngine {
                    verdict: Verdict::Refuted,
                    confidence: Confidence::Medium,
                },
                Some(valid_gate()),
            )
            .unwrap();
        store
            .render_verdict(
                &hypothesis_id,
                &FixedEngine {
                    verdict: Verdict::Inconclusive(InconclusiveKind::Fundamental {
                        frontier: "requires external replication".to_string(),
                    }),
                    confidence: Confidence::Low,
                },
                Some(valid_gate()),
            )
            .unwrap();

        let summary = store.latest_verdict_for(&hypothesis_id).unwrap().unwrap();
        assert_eq!(summary.hypothesis_id, hypothesis_id);
        assert_eq!(summary.tag, VerdictTag::InconclusiveFundamental);
        assert_eq!(summary.confidence, Confidence::Low);
        assert!(summary.created_at > 0);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn revert_hides_later_hypothesis_evidence_and_verdict_projection() {
        let path = temp_project_path("revert-horizon");
        let store = ProjectStore::init(&path, Some("Argument Demo")).unwrap();
        let checkpoint = store.create_checkpoint("before-hypothesis").unwrap();
        let hypothesis_id = record_hypothesis(&store);
        store
            .link_evidence(EvidenceLinkRequest {
                hypothesis_id: hypothesis_id.clone(),
                observation_id: None,
                source: Some("lab notebook".to_string()),
                grade: EvidenceGrade::Observed,
                stance: Stance::Supports,
                note: "Observed support".to_string(),
            })
            .unwrap();
        store
            .render_verdict(
                &hypothesis_id,
                &FixedEngine {
                    verdict: Verdict::Inconclusive(InconclusiveKind::Provisional {
                        missing: vec!["more evidence".to_string()],
                    }),
                    confidence: Confidence::Low,
                },
                None,
            )
            .unwrap();

        assert!(store
            .list_hypotheses()
            .unwrap()
            .iter()
            .any(|hypothesis| hypothesis.id == hypothesis_id));
        assert_eq!(store.evidence_for(&hypothesis_id).unwrap().len(), 1);
        assert!(store.latest_verdict_for(&hypothesis_id).unwrap().is_some());

        store.revert_to(&checkpoint.id).unwrap();

        assert!(!store
            .list_hypotheses()
            .unwrap()
            .iter()
            .any(|hypothesis| hypothesis.id == hypothesis_id));
        assert!(store.evidence_for(&hypothesis_id).unwrap().is_empty());
        assert!(store.latest_verdict_for(&hypothesis_id).unwrap().is_none());
        assert!(matches!(
            store.inspect_hypothesis(&hypothesis_id).unwrap_err(),
            StorageError::NotFound(_)
        ));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn reverted_argument_events_are_skipped_for_existing_hypothesis() {
        let path = temp_project_path("reverted-argument-events");
        let store = ProjectStore::init(&path, Some("Argument Demo")).unwrap();
        let hypothesis_id = record_hypothesis(&store);
        store
            .link_evidence(EvidenceLinkRequest {
                hypothesis_id: hypothesis_id.clone(),
                observation_id: Some("observation_before".to_string()),
                source: None,
                grade: EvidenceGrade::Observed,
                stance: Stance::Supports,
                note: "Evidence before checkpoint".to_string(),
            })
            .unwrap();
        store
            .render_verdict(
                &hypothesis_id,
                &FixedEngine {
                    verdict: Verdict::Inconclusive(InconclusiveKind::Provisional {
                        missing: vec!["first pass".to_string()],
                    }),
                    confidence: Confidence::Low,
                },
                None,
            )
            .unwrap();
        let checkpoint = store.create_checkpoint("before-later-argument").unwrap();
        store
            .link_evidence(EvidenceLinkRequest {
                hypothesis_id: hypothesis_id.clone(),
                observation_id: Some("observation_after".to_string()),
                source: None,
                grade: EvidenceGrade::Observed,
                stance: Stance::Contradicts,
                note: "Evidence after checkpoint".to_string(),
            })
            .unwrap();
        store
            .render_verdict(
                &hypothesis_id,
                &FixedEngine {
                    verdict: Verdict::Inconclusive(InconclusiveKind::Fundamental {
                        frontier: "requires external replication".to_string(),
                    }),
                    confidence: Confidence::Medium,
                },
                Some(valid_gate()),
            )
            .unwrap();

        assert_eq!(store.evidence_for(&hypothesis_id).unwrap().len(), 2);
        assert_eq!(
            store
                .latest_verdict_for(&hypothesis_id)
                .unwrap()
                .unwrap()
                .tag,
            VerdictTag::InconclusiveFundamental
        );

        store.revert_to(&checkpoint.id).unwrap();

        let evidence = store.evidence_for(&hypothesis_id).unwrap();
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].note, "Evidence before checkpoint");
        let verdict = store.latest_verdict_for(&hypothesis_id).unwrap().unwrap();
        assert_eq!(verdict.tag, VerdictTag::InconclusiveProvisional);
        assert_eq!(verdict.confidence, Confidence::Low);

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
