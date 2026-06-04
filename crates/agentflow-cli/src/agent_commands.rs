use std::path::PathBuf;

use agentflow_core::argument::{
    ClaimBasis, EvidenceGrade, EvidenceLink, EvidenceLinkRequest, RuleBasedEngine,
    SelfDeceptionGate, Stance, Verdict, VerdictReport, VerdictSummary,
};
use agentflow_core::hypothesis::{Confidence, Hypothesis, HypothesisRequest, HypothesisStatus};

use crate::cli_args::{
    EvidenceArgs, EvidenceCommand, EvidenceLinkArgs, EvidenceListArgs, HypothesisArgs,
    HypothesisCommand, HypothesisCreateArgs, HypothesisShowArgs, HypothesisTransitionArgs,
    PathJsonArgs, VerdictArgs, VerdictCommand, VerdictRenderArgs, VerdictShowArgs,
};
use crate::{last_value, project_path_from_json, CliError};

#[derive(Debug, Default)]
struct PathJsonOptions {
    path: Option<PathBuf>,
    json: bool,
}

#[derive(Debug, Default)]
struct HypothesisCreateOptions {
    project: PathJsonOptions,
    statement: Option<String>,
    origin: Option<String>,
    goal_id: Option<String>,
}

#[derive(Debug, Default)]
struct HypothesisTransitionOptions {
    project: PathJsonOptions,
    hypothesis_id: Option<String>,
    to: Option<HypothesisStatus>,
    confidence: Option<Confidence>,
}

#[derive(Debug, Default)]
struct EvidenceLinkOptions {
    project: PathJsonOptions,
    hypothesis_id: Option<String>,
    observation_id: Option<String>,
    source: Option<String>,
    grade: Option<EvidenceGrade>,
    stance: Option<Stance>,
    note: Option<String>,
}

#[derive(Debug, Default)]
struct EvidenceListOptions {
    project: PathJsonOptions,
    hypothesis_id: Option<String>,
}

#[derive(Debug, Default)]
struct VerdictHypothesisOptions {
    project: PathJsonOptions,
    hypothesis_id: Option<String>,
}

#[derive(Debug, Default)]
struct VerdictRenderOptions {
    project: PathJsonOptions,
    hypothesis_id: Option<String>,
    gate: GateOptions,
}

#[derive(Debug, Default)]
struct GateOptions {
    provided: bool,
    supports: Option<String>,
    against: Option<String>,
    alternatives: Option<String>,
    data_quality_risks: Option<String>,
    assumptions: Option<String>,
    falsifier: Option<String>,
    claim_basis: Option<ClaimBasis>,
    not_yet_claimable: Option<String>,
}

pub(crate) fn hypothesis_command(args: HypothesisArgs) -> Result<String, CliError> {
    match args.command {
        HypothesisCommand::Create(args) => hypothesis_create_command(args),
        HypothesisCommand::List(args) => hypothesis_list_command(args),
        HypothesisCommand::Show(args) => hypothesis_show_command(args),
        HypothesisCommand::Transition(args) => hypothesis_transition_command(args),
    }
}

pub(crate) fn evidence_command(args: EvidenceArgs) -> Result<String, CliError> {
    match args.command {
        EvidenceCommand::Link(args) => evidence_link_command(args),
        EvidenceCommand::List(args) => evidence_list_command(args),
    }
}

pub(crate) fn verdict_command(args: VerdictArgs) -> Result<String, CliError> {
    match args.command {
        VerdictCommand::Render(args) => verdict_render_command(args),
        VerdictCommand::Show(args) => verdict_show_command(args),
    }
}

fn hypothesis_create_command(args: HypothesisCreateArgs) -> Result<String, CliError> {
    let options = HypothesisCreateOptions::from(args);
    let statement = options.statement.ok_or_else(|| {
        CliError::InvalidArgument("hypothesis create requires --statement".to_string())
    })?;
    let origin = options.origin.ok_or_else(|| {
        CliError::InvalidArgument("hypothesis create requires --origin".to_string())
    })?;
    let related_goal_id = options.goal_id.ok_or_else(|| {
        CliError::InvalidArgument("hypothesis create requires --goal".to_string())
    })?;
    let path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let hypothesis = store.record_hypothesis(HypothesisRequest {
        statement,
        origin,
        related_goal_id,
    })?;

    if options.project.json {
        Ok(hypothesis.to_json())
    } else {
        Ok(format_hypothesis("Recorded hypothesis", &hypothesis))
    }
}

fn hypothesis_list_command(args: PathJsonArgs) -> Result<String, CliError> {
    let options = PathJsonOptions::from(args);
    let path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let hypotheses = store.list_hypotheses()?;

    if options.json {
        Ok(hypotheses_json(&hypotheses))
    } else if hypotheses.is_empty() {
        Ok("No hypotheses recorded".to_string())
    } else {
        Ok(hypotheses
            .iter()
            .map(format_hypothesis_summary)
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn hypothesis_show_command(args: HypothesisShowArgs) -> Result<String, CliError> {
    let hypothesis_id = args.hypothesis_id;
    let json = args.project.json;
    let path = project_path_from_json(args.project)?;
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let hypothesis = store.inspect_hypothesis(&hypothesis_id)?;

    if json {
        Ok(hypothesis.to_json())
    } else {
        Ok(format_hypothesis("Hypothesis", &hypothesis))
    }
}

fn hypothesis_transition_command(args: HypothesisTransitionArgs) -> Result<String, CliError> {
    let options = HypothesisTransitionOptions::try_from(args)?;
    let hypothesis_id = options.hypothesis_id.expect("clap requires hypothesis id");
    let to = options.to.ok_or_else(|| {
        CliError::InvalidArgument("hypothesis transition requires --to".to_string())
    })?;
    let confidence = options.confidence.unwrap_or(Confidence::Medium);
    let path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let hypothesis = store.transition_hypothesis(&hypothesis_id, to, confidence)?;

    if options.project.json {
        Ok(hypothesis.to_json())
    } else {
        Ok(format_hypothesis("Transitioned hypothesis", &hypothesis))
    }
}

fn evidence_link_command(args: EvidenceLinkArgs) -> Result<String, CliError> {
    let options = EvidenceLinkOptions::try_from(args)?;
    let hypothesis_id = options.hypothesis_id.ok_or_else(|| {
        CliError::InvalidArgument("evidence link requires --hypothesis".to_string())
    })?;
    let grade = options
        .grade
        .ok_or_else(|| CliError::InvalidArgument("evidence link requires --grade".to_string()))?;
    let stance = options
        .stance
        .ok_or_else(|| CliError::InvalidArgument("evidence link requires --stance".to_string()))?;
    let note = options
        .note
        .ok_or_else(|| CliError::InvalidArgument("evidence link requires --note".to_string()))?;
    let path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let link = store.link_evidence(EvidenceLinkRequest {
        hypothesis_id,
        observation_id: options.observation_id,
        source: options.source,
        grade,
        stance,
        note,
    })?;

    if options.project.json {
        Ok(link.to_json())
    } else {
        Ok(format_evidence_link("Linked evidence", &link))
    }
}

fn evidence_list_command(args: EvidenceListArgs) -> Result<String, CliError> {
    let options = EvidenceListOptions::from(args);
    let hypothesis_id = options.hypothesis_id.ok_or_else(|| {
        CliError::InvalidArgument("evidence list requires --hypothesis".to_string())
    })?;
    let path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let evidence = store.evidence_for(&hypothesis_id)?;

    if options.project.json {
        Ok(evidence_json(&evidence))
    } else if evidence.is_empty() {
        Ok(format!("No evidence linked for hypothesis {hypothesis_id}"))
    } else {
        Ok(evidence
            .iter()
            .map(format_evidence_summary)
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn verdict_render_command(args: VerdictRenderArgs) -> Result<String, CliError> {
    let options = VerdictRenderOptions::try_from(args)?;
    let hypothesis_id = options.hypothesis_id.ok_or_else(|| {
        CliError::InvalidArgument("verdict render requires --hypothesis".to_string())
    })?;
    let gate = options.gate.into_gate()?;
    let path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let engine = RuleBasedEngine;
    let report = store.render_verdict(&hypothesis_id, &engine, gate)?;

    if options.project.json {
        Ok(report.to_json())
    } else {
        Ok(format_verdict_report(&report))
    }
}

fn verdict_show_command(args: VerdictShowArgs) -> Result<String, CliError> {
    let options = VerdictHypothesisOptions::from(args);
    let hypothesis_id = options.hypothesis_id.ok_or_else(|| {
        CliError::InvalidArgument("verdict show requires --hypothesis".to_string())
    })?;
    let path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let verdict = store.latest_verdict_for(&hypothesis_id)?;

    if options.project.json {
        Ok(verdict
            .map(|summary| summary.to_json())
            .unwrap_or_else(|| "null".to_string()))
    } else {
        Ok(verdict.map_or_else(
            || format!("No verdict recorded for hypothesis {hypothesis_id}"),
            |summary| format_verdict_summary("Latest verdict", &summary),
        ))
    }
}

impl From<PathJsonArgs> for PathJsonOptions {
    fn from(args: PathJsonArgs) -> Self {
        Self {
            path: last_value(args.path),
            json: args.json,
        }
    }
}

impl From<HypothesisCreateArgs> for HypothesisCreateOptions {
    fn from(args: HypothesisCreateArgs) -> Self {
        Self {
            project: PathJsonOptions::from(args.project),
            statement: last_value(args.statement),
            origin: last_value(args.origin),
            goal_id: last_value(args.goal),
        }
    }
}

impl TryFrom<HypothesisTransitionArgs> for HypothesisTransitionOptions {
    type Error = CliError;

    fn try_from(args: HypothesisTransitionArgs) -> Result<Self, Self::Error> {
        Ok(Self {
            project: PathJsonOptions::from(args.project),
            hypothesis_id: Some(args.hypothesis_id),
            to: last_value(args.to)
                .map(|status| parse_hypothesis_status(&status))
                .transpose()?,
            confidence: last_value(args.confidence)
                .map(|confidence| parse_confidence(&confidence))
                .transpose()?,
        })
    }
}

impl TryFrom<EvidenceLinkArgs> for EvidenceLinkOptions {
    type Error = CliError;

    fn try_from(args: EvidenceLinkArgs) -> Result<Self, Self::Error> {
        Ok(Self {
            project: PathJsonOptions::from(args.project),
            hypothesis_id: last_value(args.hypothesis),
            observation_id: last_value(args.observation),
            source: last_value(args.source),
            grade: last_value(args.grade)
                .map(|grade| parse_evidence_grade(&grade))
                .transpose()?,
            stance: last_value(args.stance)
                .map(|stance| parse_stance(&stance))
                .transpose()?,
            note: last_value(args.note),
        })
    }
}

impl From<EvidenceListArgs> for EvidenceListOptions {
    fn from(args: EvidenceListArgs) -> Self {
        Self {
            project: PathJsonOptions::from(args.project),
            hypothesis_id: last_value(args.hypothesis),
        }
    }
}

impl TryFrom<VerdictRenderArgs> for VerdictRenderOptions {
    type Error = CliError;

    fn try_from(args: VerdictRenderArgs) -> Result<Self, Self::Error> {
        let gate = GateOptions {
            provided: !args.gate_supports.is_empty()
                || !args.gate_against.is_empty()
                || !args.gate_alternatives.is_empty()
                || !args.gate_data_risks.is_empty()
                || !args.gate_assumptions.is_empty()
                || !args.gate_falsifier.is_empty()
                || !args.gate_claim_basis.is_empty()
                || !args.gate_not_yet.is_empty(),
            supports: last_value(args.gate_supports),
            against: last_value(args.gate_against),
            alternatives: last_value(args.gate_alternatives),
            data_quality_risks: last_value(args.gate_data_risks),
            assumptions: last_value(args.gate_assumptions),
            falsifier: last_value(args.gate_falsifier),
            claim_basis: last_value(args.gate_claim_basis)
                .map(|value| parse_claim_basis(&value))
                .transpose()?,
            not_yet_claimable: last_value(args.gate_not_yet),
        };

        Ok(Self {
            project: PathJsonOptions::from(args.project),
            hypothesis_id: last_value(args.hypothesis),
            gate,
        })
    }
}

impl From<VerdictShowArgs> for VerdictHypothesisOptions {
    fn from(args: VerdictShowArgs) -> Self {
        Self {
            project: PathJsonOptions::from(args.project),
            hypothesis_id: last_value(args.hypothesis),
        }
    }
}

impl GateOptions {
    fn into_gate(self) -> Result<Option<SelfDeceptionGate>, CliError> {
        if !self.provided {
            return Ok(None);
        }

        Ok(Some(SelfDeceptionGate {
            supports: self
                .supports
                .ok_or_else(|| missing_gate("--gate-supports"))?,
            against: self.against.ok_or_else(|| missing_gate("--gate-against"))?,
            alternatives: self
                .alternatives
                .ok_or_else(|| missing_gate("--gate-alternatives"))?,
            data_quality_risks: self
                .data_quality_risks
                .ok_or_else(|| missing_gate("--gate-data-risks"))?,
            assumptions: self
                .assumptions
                .ok_or_else(|| missing_gate("--gate-assumptions"))?,
            falsifier: self
                .falsifier
                .ok_or_else(|| missing_gate("--gate-falsifier"))?,
            claim_basis: self
                .claim_basis
                .ok_or_else(|| missing_gate("--gate-claim-basis"))?,
            not_yet_claimable: self
                .not_yet_claimable
                .ok_or_else(|| missing_gate("--gate-not-yet"))?,
        }))
    }
}

fn missing_gate(flag: &str) -> CliError {
    CliError::InvalidArgument(format!(
        "verdict render gate requires {flag} when any gate option is provided"
    ))
}

fn parse_hypothesis_status(value: &str) -> Result<HypothesisStatus, CliError> {
    HypothesisStatus::parse(value).ok_or_else(|| {
        CliError::InvalidArgument(
            "--to must be proposed, under_test, supported, weakened, contradicted, inconclusive, or superseded"
                .to_string(),
        )
    })
}

fn parse_confidence(value: &str) -> Result<Confidence, CliError> {
    Confidence::parse(value).ok_or_else(|| {
        CliError::InvalidArgument("--confidence must be low, medium, or high".to_string())
    })
}

fn parse_evidence_grade(value: &str) -> Result<EvidenceGrade, CliError> {
    EvidenceGrade::parse(value).ok_or_else(|| {
        CliError::InvalidArgument(
            "--grade must be observed, inferred, literature_supported, hypothesis, or unsupported"
                .to_string(),
        )
    })
}

fn parse_stance(value: &str) -> Result<Stance, CliError> {
    Stance::parse(value).ok_or_else(|| {
        CliError::InvalidArgument("--stance must be supports, contradicts, or neutral".to_string())
    })
}

fn parse_claim_basis(value: &str) -> Result<ClaimBasis, CliError> {
    match value {
        "observed" => Ok(ClaimBasis::Observed),
        "inferred" => Ok(ClaimBasis::StatisticallyInferred),
        "speculative" => Ok(ClaimBasis::Speculative),
        _ => Err(CliError::InvalidArgument(
            "--gate-claim-basis must be observed, inferred, or speculative".to_string(),
        )),
    }
}

fn hypotheses_json(hypotheses: &[Hypothesis]) -> String {
    let items = hypotheses
        .iter()
        .map(Hypothesis::to_json)
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"schema_version\":\"agentflow.hypotheses.v0\",\"hypotheses\":[{items}]}}")
}

fn evidence_json(evidence: &[EvidenceLink]) -> String {
    let items = evidence
        .iter()
        .map(EvidenceLink::to_json)
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"schema_version\":\"agentflow.evidence.v0\",\"evidence\":[{items}]}}")
}

fn format_hypothesis(heading: &str, hypothesis: &Hypothesis) -> String {
    format!(
        "{heading}\nId: {}\nStatement: {}\nOrigin: {}\nGoal: {}\nStatus: {}\nConfidence: {}\nCreated: {}\nUpdated: {}",
        hypothesis.id,
        hypothesis.statement,
        hypothesis.origin,
        hypothesis.related_goal_id,
        hypothesis.status,
        hypothesis.confidence,
        hypothesis.created_at,
        hypothesis.updated_at
    )
}

fn format_hypothesis_summary(hypothesis: &Hypothesis) -> String {
    format!(
        "{} [{}/{}] {}\n  origin: {}\n  goal: {}",
        hypothesis.id,
        hypothesis.status,
        hypothesis.confidence,
        hypothesis.statement,
        hypothesis.origin,
        hypothesis.related_goal_id
    )
}

fn format_evidence_link(heading: &str, link: &EvidenceLink) -> String {
    format!(
        "{heading}\nId: {}\nHypothesis: {}\nObservation: {}\nSource: {}\nGrade: {}\nStance: {}\nNote: {}\nCreated: {}",
        link.id,
        link.hypothesis_id,
        link.observation_id.as_deref().unwrap_or("none"),
        link.source.as_deref().unwrap_or("none"),
        link.grade,
        link.stance,
        link.note,
        link.created_at
    )
}

fn format_evidence_summary(link: &EvidenceLink) -> String {
    format!(
        "{} [{}/{}] {}\n  observation: {}\n  source: {}",
        link.id,
        link.grade,
        link.stance,
        link.note,
        link.observation_id.as_deref().unwrap_or("none"),
        link.source.as_deref().unwrap_or("none")
    )
}

fn format_verdict_report(report: &VerdictReport) -> String {
    format!(
        "Verdict\nHypothesis: {}\nVerdict: {}\nConfidence: {}\nRationale: {}\nSupporting evidence: {}\nContradicting evidence: {}",
        report.hypothesis_id,
        verdict_label(&report.verdict),
        report.confidence,
        report.rationale,
        report.supporting.len(),
        report.contradicting.len()
    )
}

fn format_verdict_summary(heading: &str, summary: &VerdictSummary) -> String {
    format!(
        "{heading}\nHypothesis: {}\nVerdict: {}\nConfidence: {}\nCreated: {}",
        summary.hypothesis_id, summary.tag, summary.confidence, summary.created_at
    )
}

fn verdict_label(verdict: &Verdict) -> &'static str {
    match verdict {
        Verdict::Affirmed => "affirmed",
        Verdict::Refuted => "refuted",
        Verdict::Inconclusive(kind) => match kind {
            agentflow_core::argument::InconclusiveKind::Provisional { .. } => {
                "inconclusive_provisional"
            }
            agentflow_core::argument::InconclusiveKind::Fundamental { .. } => {
                "inconclusive_fundamental"
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;
    use crate::run;

    fn args(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    fn temp_project_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentflow-cli-c1-{test_name}-{}-{}",
            std::process::id(),
            agentflow_core::storage::now_unix_seconds()
        ))
    }

    fn init_project(path: &std::path::Path) {
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "C1 Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
    }

    fn create_hypothesis(path: &std::path::Path) -> String {
        let output = run(args(&[
            "agentflow",
            "hypothesis",
            "create",
            "--statement",
            "Marker A supports pathway B",
            "--origin",
            "test",
            "--goal",
            "goal_c1",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        output
            .split("\"id\":\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap()
            .to_string()
    }

    fn link_supporting_observed_evidence(path: &std::path::Path, hypothesis_id: &str) {
        run(args(&[
            "agentflow",
            "evidence",
            "link",
            "--hypothesis",
            hypothesis_id,
            "--grade",
            "observed",
            "--stance",
            "supports",
            "--note",
            "Observed validation supports the claim.",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
    }

    #[test]
    fn hypothesis_commands_work_with_json_and_explicit_path() {
        let path = temp_project_path("hypothesis-json-path");
        init_project(&path);
        let hypothesis_id = create_hypothesis(&path);

        let list = run(args(&[
            "agentflow",
            "hypothesis",
            "list",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(list.contains("\"schema_version\":\"agentflow.hypotheses.v0\""));
        assert!(list.contains(&hypothesis_id));

        let show = run(args(&[
            "agentflow",
            "hypothesis",
            "show",
            &hypothesis_id,
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(show.contains("\"statement\":\"Marker A supports pathway B\""));

        let transition = run(args(&[
            "agentflow",
            "hypothesis",
            "transition",
            &hypothesis_id,
            "--to",
            "under_test",
            "--confidence",
            "medium",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(transition.contains("\"status\":\"under_test\""));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn evidence_and_verdict_commands_work_with_json_and_gate() {
        let path = temp_project_path("evidence-verdict");
        init_project(&path);
        let hypothesis_id = create_hypothesis(&path);

        let evidence = run(args(&[
            "agentflow",
            "evidence",
            "link",
            "--hypothesis",
            &hypothesis_id,
            "--grade",
            "observed",
            "--stance",
            "supports",
            "--note",
            "Observed validation supports the claim.",
            "--observation",
            "observation_1",
            "--source",
            "local test",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(evidence.contains("\"grade\":\"observed\""));
        assert!(evidence.contains("\"stance\":\"supports\""));

        let list = run(args(&[
            "agentflow",
            "evidence",
            "list",
            "--hypothesis",
            &hypothesis_id,
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(list.contains("\"schema_version\":\"agentflow.evidence.v0\""));
        assert!(list.contains("\"source\":\"local test\""));

        let verdict = run(args(&[
            "agentflow",
            "verdict",
            "render",
            "--hypothesis",
            &hypothesis_id,
            "--gate-supports",
            "Observed support is present.",
            "--gate-against",
            "Contradictory evidence was checked.",
            "--gate-alternatives",
            "Alternative pathway remains possible.",
            "--gate-data-risks",
            "Small test fixture.",
            "--gate-assumptions",
            "Fixture represents real input shape.",
            "--gate-falsifier",
            "Independent contradiction would refute it.",
            "--gate-claim-basis",
            "observed",
            "--gate-not-yet",
            "Not claimable as external truth.",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(verdict.contains("\"verdict\":\"affirmed\""));
        assert!(verdict.contains("\"confidence\":\"medium\""));

        let show = run(args(&[
            "agentflow",
            "verdict",
            "show",
            "--hypothesis",
            &hypothesis_id,
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(show.contains("\"tag\":\"affirmed\""));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn required_cli_errors_are_reported() {
        let path = temp_project_path("errors");
        init_project(&path);
        let hypothesis_id = create_hypothesis(&path);

        let transition_error = run(args(&[
            "agentflow",
            "hypothesis",
            "transition",
            &hypothesis_id,
            "--to",
            "supported",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(transition_error
            .message()
            .contains("cannot transition from proposed to supported"));

        let evidence_error = run(args(&[
            "agentflow",
            "evidence",
            "link",
            "--hypothesis",
            &hypothesis_id,
            "--grade",
            "observed",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert_eq!(evidence_error.message(), "evidence link requires --stance");

        link_supporting_observed_evidence(&path, &hypothesis_id);
        let gate_error = run(args(&[
            "agentflow",
            "verdict",
            "render",
            "--hypothesis",
            &hypothesis_id,
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert_eq!(
            gate_error.message(),
            "strong verdict requires self-deception gate"
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn verdict_show_reports_absent_summary() {
        let path = temp_project_path("verdict-show-empty");
        init_project(&path);
        let hypothesis_id = create_hypothesis(&path);

        let output = run(args(&[
            "agentflow",
            "verdict",
            "show",
            "--hypothesis",
            &hypothesis_id,
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert_eq!(
            output,
            format!("No verdict recorded for hypothesis {hypothesis_id}")
        );

        let json = run(args(&[
            "agentflow",
            "verdict",
            "show",
            "--hypothesis",
            &hypothesis_id,
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert_eq!(json, "null");

        let _ = std::fs::remove_dir_all(path);
    }
}
