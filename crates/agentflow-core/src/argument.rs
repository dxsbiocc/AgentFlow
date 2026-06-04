use std::fmt;

use rusqlite::params;
use serde::de::{self, DeserializeOwned};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::domain::ToolMaturity;
use crate::hypothesis::Confidence;
use crate::storage::{EventRecord, ProjectStore, StorageError};

const EVIDENCE_LINKED_EVENT: &str = "argument.evidence_linked";
const VERDICT_RENDERED_EVENT: &str = "argument.verdict_rendered";
const AFFIRM_MARGIN: i32 = 3;
const REFUTE_MARGIN: i32 = 3;
const STRONG_MARGIN: i32 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        serde_json::to_string(self).expect("evidence link serializes to JSON")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
            Self::Affirmed | Self::Refuted => serde_json::to_string(&VerdictTextPayload {
                verdict: self.as_str().to_string(),
            })
            .expect("verdict serializes to JSON"),
            Self::Inconclusive(InconclusiveKind::Provisional { missing }) => {
                serde_json::to_string(&ProvisionalVerdictPayload {
                    verdict: "inconclusive".to_string(),
                    inconclusive_kind: "provisional".to_string(),
                    missing: missing.clone(),
                })
                .expect("provisional verdict serializes to JSON")
            }
            Self::Inconclusive(InconclusiveKind::Fundamental { frontier }) => {
                serde_json::to_string(&FundamentalVerdictPayload {
                    verdict: "inconclusive".to_string(),
                    inconclusive_kind: "fundamental".to_string(),
                    frontier: frontier.clone(),
                })
                .expect("fundamental verdict serializes to JSON")
            }
        }
    }

    pub fn from_json(json: &str) -> Option<Self> {
        let payload: VerdictPayload = serde_json::from_str(json).ok()?;
        match payload.verdict.as_str() {
            "affirmed" => Some(Self::Affirmed),
            "refuted" => Some(Self::Refuted),
            "inconclusive" => match payload.inconclusive_kind?.as_str() {
                "provisional" => Some(Self::Inconclusive(InconclusiveKind::Provisional {
                    missing: payload.missing?,
                })),
                "fundamental" => Some(Self::Inconclusive(InconclusiveKind::Fundamental {
                    frontier: payload.frontier?,
                })),
                _ => None,
            },
            _ => None,
        }
    }
}

#[derive(Debug, Serialize)]
struct VerdictTextPayload {
    verdict: String,
}

#[derive(Debug, Serialize)]
struct ProvisionalVerdictPayload {
    verdict: String,
    inconclusive_kind: String,
    missing: Vec<String>,
}

#[derive(Debug, Serialize)]
struct FundamentalVerdictPayload {
    verdict: String,
    inconclusive_kind: String,
    frontier: String,
}

#[derive(Debug, Deserialize)]
struct VerdictPayload {
    verdict: String,
    inconclusive_kind: Option<String>,
    missing: Option<Vec<String>>,
    frontier: Option<String>,
}

fn report_verdict_from_parts(
    verdict: &str,
    inconclusive_kind: Option<&str>,
    missing: Vec<String>,
    frontier: Option<&str>,
) -> Result<Verdict, String> {
    match verdict {
        "affirmed" => Ok(Verdict::Affirmed),
        "refuted" => Ok(Verdict::Refuted),
        "inconclusive" => match inconclusive_kind {
            Some("provisional") => Ok(Verdict::Inconclusive(InconclusiveKind::Provisional {
                missing,
            })),
            Some("fundamental") => frontier
                .map(|frontier| {
                    Verdict::Inconclusive(InconclusiveKind::Fundamental {
                        frontier: frontier.to_string(),
                    })
                })
                .ok_or_else(|| "fundamental verdict missing frontier".to_string()),
            Some(kind) => Err(format!("invalid inconclusive kind {kind}")),
            None => Err("inconclusive verdict missing inconclusive_kind".to_string()),
        },
        other => Err(format!("invalid verdict {other}")),
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
        serde_json::to_string(self).expect("verdict report serializes to JSON")
    }
}

impl Serialize for VerdictReport {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        VerdictReportPayload::from(self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for VerdictReport {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let payload = VerdictReportInput::deserialize(deserializer)?;
        let verdict = report_verdict_from_parts(
            &payload.verdict,
            payload.inconclusive_kind.as_deref(),
            payload.missing,
            payload.frontier.as_deref(),
        )
        .map_err(de::Error::custom)?;

        Ok(Self {
            hypothesis_id: payload.hypothesis_id,
            verdict,
            confidence: payload.confidence,
            supporting: payload.supporting,
            contradicting: payload.contradicting,
            rationale: payload.rationale,
        })
    }
}

#[derive(Debug, Serialize)]
struct VerdictReportPayload<'a> {
    hypothesis_id: &'a str,
    verdict: &'static str,
    inconclusive_kind: Option<&'static str>,
    missing: Vec<String>,
    frontier: Option<&'a str>,
    confidence: Confidence,
    rationale: &'a str,
    supporting: &'a [EvidenceLink],
    contradicting: &'a [EvidenceLink],
}

impl<'a> From<&'a VerdictReport> for VerdictReportPayload<'a> {
    fn from(report: &'a VerdictReport) -> Self {
        let (inconclusive_kind, missing, frontier) = match &report.verdict {
            Verdict::Inconclusive(InconclusiveKind::Provisional { missing }) => {
                (Some("provisional"), missing.clone(), None)
            }
            Verdict::Inconclusive(InconclusiveKind::Fundamental { frontier }) => {
                (Some("fundamental"), Vec::new(), Some(frontier.as_str()))
            }
            Verdict::Affirmed | Verdict::Refuted => (None, Vec::new(), None),
        };

        Self {
            hypothesis_id: &report.hypothesis_id,
            verdict: report.verdict.as_str(),
            inconclusive_kind,
            missing,
            frontier,
            confidence: report.confidence,
            rationale: &report.rationale,
            supporting: &report.supporting,
            contradicting: &report.contradicting,
        }
    }
}

#[derive(Debug, Deserialize)]
struct VerdictReportInput {
    hypothesis_id: String,
    verdict: String,
    inconclusive_kind: Option<String>,
    #[serde(default)]
    missing: Vec<String>,
    frontier: Option<String>,
    confidence: Confidence,
    rationale: String,
    #[serde(default)]
    supporting: Vec<EvidenceLink>,
    #[serde(default)]
    contradicting: Vec<EvidenceLink>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerdictSummary {
    pub hypothesis_id: String,
    pub tag: VerdictTag,
    pub confidence: Confidence,
    pub frontier: Option<String>,
    pub created_at: i64,
}

impl VerdictSummary {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("verdict summary serializes to JSON")
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
        if !self
            .source_inferred_params_for_observation(observation_id)?
            .is_empty()
        {
            return Ok(EvidenceGrade::Inferred);
        }
        if self.source_tool_maturity_for_observation(observation_id)?
            == Some(ToolMaturity::Exploratory)
        {
            return Ok(EvidenceGrade::Inferred);
        }

        Ok(request.grade)
    }

    fn source_inferred_params_for_observation(
        &self,
        observation_id: &str,
    ) -> Result<Vec<(String, String)>, StorageError> {
        let observation_id = observation_id.trim();
        if observation_id.is_empty() {
            return Ok(Vec::new());
        }

        let observation = match self.inspect_observation(observation_id) {
            Ok(observation) => observation,
            Err(StorageError::NotFound(_)) => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let (Some(flow_id), Some(step_id)) = (observation.flow_id, observation.step_id) else {
            return Ok(Vec::new());
        };

        self.inferred_params_for_step(&flow_id, &step_id)
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
            if payload_hypothesis_id_from_json(&event_id, &payload_json)?.as_str() == hypothesis_id
            {
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
            if payload_hypothesis_id_from_json(&event_id, &payload_json)?.as_str() == hypothesis_id
            {
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
    serde_json::to_string(&EvidenceLinkedPayload {
        hypothesis_id: request.hypothesis_id.trim().to_string(),
        observation_id: trimmed_non_empty(request.observation_id.as_deref()),
        source: trimmed_non_empty(request.source.as_deref()),
        grade: request.grade,
        stance: request.stance,
        note: request.note.trim().to_string(),
    })
    .expect("evidence linked payload serializes to JSON")
}

fn evidence_from_event(
    id: String,
    payload_json: &str,
    created_at: i64,
) -> Result<EvidenceLink, StorageError> {
    let payload: EvidenceLinkedPayload = argument_payload_from_json(&id, payload_json)?;
    Ok(EvidenceLink {
        id,
        hypothesis_id: payload.hypothesis_id,
        observation_id: payload.observation_id,
        source: payload.source,
        grade: payload.grade,
        stance: payload.stance,
        note: payload.note,
        created_at,
    })
}

fn verdict_rendered_payload_json(
    report: &VerdictReport,
    gate: Option<&SelfDeceptionGate>,
) -> String {
    serde_json::to_string(&VerdictRenderedPayload {
        hypothesis_id: report.hypothesis_id.clone(),
        verdict: verdict_payload_text(&report.verdict),
        confidence: report.confidence,
        frontier: verdict_frontier(&report.verdict).map(ToString::to_string),
        rationale: report.rationale.clone(),
        gate: gate.map(SelfDeceptionGatePayload::from),
    })
    .expect("verdict rendered payload serializes to JSON")
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

fn verdict_summary_from_event(
    event_id: String,
    payload_json: &str,
    created_at: i64,
) -> Result<VerdictSummary, StorageError> {
    let payload: VerdictRenderedSummaryPayload =
        argument_payload_from_json(&event_id, payload_json)?;
    let tag = VerdictTag::parse(&payload.verdict).ok_or_else(|| {
        StorageError::InvalidInput(format!(
            "argument event {event_id} has invalid verdict {}",
            payload.verdict
        ))
    })?;

    Ok(VerdictSummary {
        hypothesis_id: payload.hypothesis_id,
        tag,
        confidence: payload.confidence,
        frontier: payload.frontier,
        created_at,
    })
}

#[derive(Debug, Serialize, Deserialize)]
struct EvidenceLinkedPayload {
    hypothesis_id: String,
    observation_id: Option<String>,
    source: Option<String>,
    grade: EvidenceGrade,
    stance: Stance,
    note: String,
}

#[derive(Debug, Serialize)]
struct VerdictRenderedPayload {
    hypothesis_id: String,
    verdict: String,
    confidence: Confidence,
    frontier: Option<String>,
    rationale: String,
    gate: Option<SelfDeceptionGatePayload>,
}

#[derive(Debug, Deserialize)]
struct VerdictRenderedSummaryPayload {
    hypothesis_id: String,
    verdict: String,
    confidence: Confidence,
    frontier: Option<String>,
}

#[derive(Debug, Serialize)]
struct SelfDeceptionGatePayload {
    supports: String,
    against: String,
    alternatives: String,
    data_quality_risks: String,
    assumptions: String,
    falsifier: String,
    claim_basis: ClaimBasis,
    not_yet_claimable: String,
}

impl From<&SelfDeceptionGate> for SelfDeceptionGatePayload {
    fn from(gate: &SelfDeceptionGate) -> Self {
        Self {
            supports: gate.supports.trim().to_string(),
            against: gate.against.trim().to_string(),
            alternatives: gate.alternatives.trim().to_string(),
            data_quality_risks: gate.data_quality_risks.trim().to_string(),
            assumptions: gate.assumptions.trim().to_string(),
            falsifier: gate.falsifier.trim().to_string(),
            claim_basis: gate.claim_basis,
            not_yet_claimable: gate.not_yet_claimable.trim().to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct HypothesisIdPayload {
    hypothesis_id: String,
}

fn payload_hypothesis_id_from_json(
    event_id: &str,
    payload_json: &str,
) -> Result<String, StorageError> {
    let payload: HypothesisIdPayload = argument_payload_from_json(event_id, payload_json)?;
    Ok(payload.hypothesis_id)
}

fn argument_payload_from_json<T>(event_id: &str, payload_json: &str) -> Result<T, StorageError>
where
    T: DeserializeOwned,
{
    serde_json::from_str(payload_json).map_err(|err| {
        StorageError::InvalidInput(format!(
            "argument event {event_id} has invalid payload: {err}"
        ))
    })
}

fn trimmed_non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|inner| !inner.is_empty())
        .map(ToString::to_string)
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
    fn enum_json_strings_match_display_contract() {
        for grade in [
            EvidenceGrade::Observed,
            EvidenceGrade::Inferred,
            EvidenceGrade::LiteratureSupported,
            EvidenceGrade::Hypothesis,
            EvidenceGrade::Unsupported,
        ] {
            assert_eq!(
                serde_json::to_string(&grade).unwrap(),
                format!("\"{}\"", grade.as_str())
            );
        }
        for stance in [Stance::Supports, Stance::Contradicts, Stance::Neutral] {
            assert_eq!(
                serde_json::to_string(&stance).unwrap(),
                format!("\"{}\"", stance.as_str())
            );
        }
        for basis in [
            ClaimBasis::Observed,
            ClaimBasis::StatisticallyInferred,
            ClaimBasis::Speculative,
        ] {
            assert_eq!(
                serde_json::to_string(&basis).unwrap(),
                format!("\"{}\"", basis.as_str())
            );
        }
        for tag in [
            VerdictTag::Affirmed,
            VerdictTag::Refuted,
            VerdictTag::InconclusiveProvisional,
            VerdictTag::InconclusiveFundamental,
        ] {
            assert_eq!(
                serde_json::to_string(&tag).unwrap(),
                format!("\"{}\"", tag.as_str())
            );
        }
    }

    #[test]
    fn json_outputs_match_legacy_bytes() {
        let link = EvidenceLink {
            id: "event_1".to_string(),
            hypothesis_id: "hypothesis_1".to_string(),
            observation_id: Some("observation_1".to_string()),
            source: Some("Source \"A\"\n".to_string()),
            grade: EvidenceGrade::LiteratureSupported,
            stance: Stance::Contradicts,
            note: "Quote \" and newline\nslash \\ tab\t".to_string(),
            created_at: 42,
        };
        assert_eq!(
            link.to_json(),
            "{\"id\":\"event_1\",\"hypothesis_id\":\"hypothesis_1\",\"observation_id\":\"observation_1\",\"source\":\"Source \\\"A\\\"\\n\",\"grade\":\"literature_supported\",\"stance\":\"contradicts\",\"note\":\"Quote \\\" and newline\\nslash \\\\ tab\\t\",\"created_at\":42}"
        );

        assert_eq!(Verdict::Affirmed.to_json(), "{\"verdict\":\"affirmed\"}");
        assert_eq!(Verdict::Refuted.to_json(), "{\"verdict\":\"refuted\"}");
        assert_eq!(
            Verdict::Inconclusive(InconclusiveKind::Provisional {
                missing: vec![
                    "need \"observed\"".to_string(),
                    "replicate\nagain".to_string(),
                ],
            })
            .to_json(),
            "{\"verdict\":\"inconclusive\",\"inconclusive_kind\":\"provisional\",\"missing\":[\"need \\\"observed\\\"\",\"replicate\\nagain\"]}"
        );
        assert_eq!(
            Verdict::Inconclusive(InconclusiveKind::Fundamental {
                frontier: "external \"replication\"\nfrontier".to_string(),
            })
            .to_json(),
            "{\"verdict\":\"inconclusive\",\"inconclusive_kind\":\"fundamental\",\"frontier\":\"external \\\"replication\\\"\\nfrontier\"}"
        );

        let supporting = evidence_link("support_1", EvidenceGrade::Observed, Stance::Supports);
        let contradicting =
            evidence_link("contradict_1", EvidenceGrade::Inferred, Stance::Contradicts);
        let provisional_report = VerdictReport {
            hypothesis_id: "hypothesis_1".to_string(),
            verdict: Verdict::Inconclusive(InconclusiveKind::Provisional {
                missing: vec!["more evidence".to_string()],
            }),
            confidence: Confidence::Low,
            supporting: vec![supporting],
            contradicting: vec![contradicting],
            rationale: "Need \"more\"\n".to_string(),
        };
        assert_eq!(
            provisional_report.to_json(),
            "{\"hypothesis_id\":\"hypothesis_1\",\"verdict\":\"inconclusive\",\"inconclusive_kind\":\"provisional\",\"missing\":[\"more evidence\"],\"frontier\":null,\"confidence\":\"low\",\"rationale\":\"Need \\\"more\\\"\\n\",\"supporting\":[{\"id\":\"support_1\",\"hypothesis_id\":\"hypothesis_1\",\"observation_id\":null,\"source\":null,\"grade\":\"observed\",\"stance\":\"supports\",\"note\":\"supports via observed\",\"created_at\":1}],\"contradicting\":[{\"id\":\"contradict_1\",\"hypothesis_id\":\"hypothesis_1\",\"observation_id\":null,\"source\":null,\"grade\":\"inferred\",\"stance\":\"contradicts\",\"note\":\"contradicts via inferred\",\"created_at\":1}]}"
        );
        assert_eq!(
            super::verdict_rendered_payload_json(&provisional_report, None),
            "{\"hypothesis_id\":\"hypothesis_1\",\"verdict\":\"inconclusive_provisional\",\"confidence\":\"low\",\"frontier\":null,\"rationale\":\"Need \\\"more\\\"\\n\",\"gate\":null}"
        );

        let fundamental_report = VerdictReport {
            hypothesis_id: "hypothesis_1".to_string(),
            verdict: Verdict::Inconclusive(InconclusiveKind::Fundamental {
                frontier: "external replication".to_string(),
            }),
            confidence: Confidence::Medium,
            supporting: Vec::new(),
            contradicting: Vec::new(),
            rationale: "Frontier".to_string(),
        };
        assert_eq!(
            fundamental_report.to_json(),
            "{\"hypothesis_id\":\"hypothesis_1\",\"verdict\":\"inconclusive\",\"inconclusive_kind\":\"fundamental\",\"missing\":[],\"frontier\":\"external replication\",\"confidence\":\"medium\",\"rationale\":\"Frontier\",\"supporting\":[],\"contradicting\":[]}"
        );
        assert_eq!(
            super::verdict_rendered_payload_json(&fundamental_report, Some(&valid_gate())),
            "{\"hypothesis_id\":\"hypothesis_1\",\"verdict\":\"inconclusive_fundamental\",\"confidence\":\"medium\",\"frontier\":\"external replication\",\"rationale\":\"Frontier\",\"gate\":{\"supports\":\"Observed evidence supports the claim\",\"against\":\"Contradictory evidence has been checked\",\"alternatives\":\"Alternative explanations remain less consistent\",\"data_quality_risks\":\"Sampling bias is limited by replication\",\"assumptions\":\"Measurements are comparable across runs\",\"falsifier\":\"A replicated contradiction would overturn this claim\",\"claim_basis\":\"observed\",\"not_yet_claimable\":\"No causal mechanism is claimed yet\"}}"
        );

        assert_eq!(
            super::evidence_linked_payload_json(&EvidenceLinkRequest {
                hypothesis_id: " hypothesis_1 ".to_string(),
                observation_id: Some(" observation_1 ".to_string()),
                source: Some(" Source \"A\" ".to_string()),
                grade: EvidenceGrade::Observed,
                stance: Stance::Supports,
                note: " Note\n ".to_string(),
            }),
            "{\"hypothesis_id\":\"hypothesis_1\",\"observation_id\":\"observation_1\",\"source\":\"Source \\\"A\\\"\",\"grade\":\"observed\",\"stance\":\"supports\",\"note\":\"Note\"}"
        );

        assert_eq!(
            (super::VerdictSummary {
                hypothesis_id: "hypothesis_1".to_string(),
                tag: VerdictTag::InconclusiveFundamental,
                confidence: Confidence::High,
                frontier: Some("frontier\n".to_string()),
                created_at: 99,
            })
            .to_json(),
            "{\"hypothesis_id\":\"hypothesis_1\",\"tag\":\"inconclusive_fundamental\",\"confidence\":\"high\",\"frontier\":\"frontier\\n\",\"created_at\":99}"
        );
    }

    #[test]
    fn legacy_handwritten_payloads_parse_with_json_whitespace_and_ordering() {
        let path = temp_project_path("legacy-payload");
        let store = ProjectStore::init(&path, Some("Argument Demo")).unwrap();
        let hypothesis_id = record_hypothesis(&store);

        store
            .append_event(crate::storage::EventRecord {
                flow_id: None,
                step_id: None,
                run_id: None,
                event_type: super::EVIDENCE_LINKED_EVENT.to_string(),
                payload_json: format!(
                    r#"{{
                        "note": "Legacy \"evidence\"\nparses",
                        "stance": "contradicts",
                        "grade": "inferred",
                        "source": null,
                        "observation_id": "observation_legacy",
                        "hypothesis_id": "{hypothesis_id}"
                    }}"#
                ),
            })
            .unwrap();
        store
            .append_event(crate::storage::EventRecord {
                flow_id: None,
                step_id: None,
                run_id: None,
                event_type: super::VERDICT_RENDERED_EVENT.to_string(),
                payload_json: format!(
                    r#"{{
                        "gate": null,
                        "rationale": "Legacy verdict parses",
                        "frontier": "external replication",
                        "confidence": "medium",
                        "verdict": "inconclusive_fundamental",
                        "hypothesis_id": "{hypothesis_id}"
                    }}"#
                ),
            })
            .unwrap();

        let evidence = store.evidence_for(&hypothesis_id).unwrap();
        assert_eq!(evidence.len(), 1);
        assert_eq!(
            evidence[0].observation_id.as_deref(),
            Some("observation_legacy")
        );
        assert_eq!(evidence[0].source, None);
        assert_eq!(evidence[0].grade, EvidenceGrade::Inferred);
        assert_eq!(evidence[0].stance, Stance::Contradicts);
        assert_eq!(evidence[0].note, "Legacy \"evidence\"\nparses");

        let summary = store.latest_verdict_for(&hypothesis_id).unwrap().unwrap();
        assert_eq!(summary.tag, VerdictTag::InconclusiveFundamental);
        assert_eq!(summary.confidence, Confidence::Medium);
        assert_eq!(summary.frontier.as_deref(), Some("external replication"));

        let parsed_link: EvidenceLink = serde_json::from_str(
            r#"{
                "created_at": 7,
                "note": "Legacy domain link",
                "stance": "supports",
                "grade": "observed",
                "source": "manual",
                "observation_id": null,
                "hypothesis_id": "hypothesis_legacy",
                "id": "event_legacy"
            }"#,
        )
        .unwrap();
        assert_eq!(parsed_link.id, "event_legacy");
        assert_eq!(parsed_link.grade, EvidenceGrade::Observed);
        assert_eq!(parsed_link.source.as_deref(), Some("manual"));

        assert_eq!(
            Verdict::from_json(
                r#"{
                    "missing": ["observed support", "replication"],
                    "inconclusive_kind": "provisional",
                    "verdict": "inconclusive"
                }"#
            )
            .unwrap(),
            Verdict::Inconclusive(InconclusiveKind::Provisional {
                missing: vec!["observed support".to_string(), "replication".to_string()],
            })
        );

        let _ = std::fs::remove_dir_all(path);
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
    fn observed_grade_is_capped_for_non_exploratory_observation_with_inferred_params() {
        let path = temp_project_path("grade-cap-inferred-param");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Argument Demo")).unwrap();
        let observation_id = tool_observation(
            &store,
            &path,
            "verified_param_flow",
            "verified_param_tool",
            "verified",
        );
        let hypothesis_id = record_hypothesis(&store);
        store
            .append_event(crate::storage::EventRecord {
                flow_id: Some("verified_param_flow".to_string()),
                step_id: Some("produce".to_string()),
                run_id: None,
                event_type: "agent.params_inferred".to_string(),
                payload_json: r#"{"flow_id":"verified_param_flow","step_id":"produce","hypothesis_id":"hypothesis_1","params":[{"name":"gene","value":"THRSP"}]}"#
                    .to_string(),
            })
            .unwrap();

        let evidence = store
            .link_evidence(EvidenceLinkRequest {
                hypothesis_id: hypothesis_id.clone(),
                observation_id: Some(observation_id),
                source: None,
                grade: EvidenceGrade::Observed,
                stance: Stance::Supports,
                note: "Verified source with inferred parameter".to_string(),
            })
            .unwrap();

        assert_eq!(
            store
                .inferred_params_for_step("verified_param_flow", "step:verified_param_flow/produce")
                .unwrap(),
            vec![("gene".to_string(), "THRSP".to_string())]
        );
        assert_eq!(evidence.grade, EvidenceGrade::Inferred);
        assert_eq!(
            store.evidence_for(&hypothesis_id).unwrap()[0].grade,
            EvidenceGrade::Inferred
        );

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn observed_grade_is_not_capped_for_non_exploratory_observation_without_inferred_params() {
        let path = temp_project_path("grade-no-cap-no-inferred-param");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Argument Demo")).unwrap();
        let observation_id = tool_observation(
            &store,
            &path,
            "verified_clean_flow",
            "verified_clean",
            "verified",
        );
        let hypothesis_id = record_hypothesis(&store);

        let evidence = store
            .link_evidence(EvidenceLinkRequest {
                hypothesis_id: hypothesis_id.clone(),
                observation_id: Some(observation_id),
                source: None,
                grade: EvidenceGrade::Observed,
                stance: Stance::Supports,
                note: "Verified source without inferred parameter".to_string(),
            })
            .unwrap();

        assert_eq!(
            store
                .inferred_params_for_step("verified_clean_flow", "produce")
                .unwrap(),
            Vec::<(String, String)>::new()
        );
        assert_eq!(evidence.grade, EvidenceGrade::Observed);
        assert_eq!(
            store.evidence_for(&hypothesis_id).unwrap()[0].grade,
            EvidenceGrade::Observed
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
