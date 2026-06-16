use std::fs;
use std::path::{Path, PathBuf};

use agentflow_core::agent::{
    AppliedAction, ApplyConfig, CohortInferer, NoopOutputGroundingScorer, NoopParamInferer,
    NoopRelevanceScorer,
};
use agentflow_core::argument::{
    ClaimBasis, EvidenceGrade, EvidenceLinkRequest, InconclusiveKind, RuleBasedEngine,
    SelfDeceptionGate, Stance, Verdict,
};
use agentflow_core::storage::{ArtifactImportMode, ArtifactImportRequest, ProjectStore, ToolSpec};

struct TempProject {
    path: PathBuf,
}

impl TempProject {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "agentflow-cohort-param-infer-cap-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temp project should be created");
        Self { path }
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct StubCohortInferer {
    value: String,
}

impl CohortInferer for StubCohortInferer {
    fn infer_cohort(&self, _hypothesis_statement: &str) -> Option<String> {
        Some(self.value.clone())
    }
}

fn write_marker_tool(project: &Path) {
    let script_path = project.join("marker_emit.sh");
    fs::write(
        &script_path,
        r#"set -eu
cat "$AGENTFLOW_INPUT_EXPRESSION_TABLE" >/dev/null
printf '# Marker report\nSegment: %s\nscore: 0.90\n' "$AGENTFLOW_PARAM_SEGMENT" > "$AGENTFLOW_OUTPUT_MARKER_REPORT"
printf 'marker_emit ok\n'
"#,
    )
    .expect("tool script should be written");

    let spec = ToolSpec::from_simple_yaml(&format!(
        r#"
schema_version: agentflow.tool.v0
namespace: synthetic
name: marker_emit
version: 0.1.0
maturity: verified
description: Emit a deterministic synthetic marker report for target segment validation.
inputs:
  expression_table:
    type: ExpressionTable
    required: true
params:
  segment:
    type: string
    required: true
    pattern: "^[a-z0-9_]+$"
    infer: cohort
outputs:
  marker_report:
    type: Markdown
    observer: marker_report
runtime:
  backend: local
  command:
    - /bin/sh
    - {}
"#,
        script_path.display()
    ))
    .expect("tool spec should parse");
    project_store(project).register_tool(spec).unwrap();
}

fn project_store(project: &Path) -> ProjectStore {
    ProjectStore::open(project).expect("project store should open")
}

fn import_expression_table(project: &Path) {
    let source_path = project.join("expression.tsv");
    fs::write(
        &source_path,
        "\
sample\tmarker
s1\t0.10
s2\t0.20
s3\t0.80
s4\t0.90
",
    )
    .expect("expression table should be written");

    project_store(project)
        .import_artifact(ArtifactImportRequest {
            source_path,
            artifact_type: "ExpressionTable".to_string(),
            mode: ArtifactImportMode::Reference,
        })
        .expect("expression table should import");
}

fn gate() -> SelfDeceptionGate {
    SelfDeceptionGate {
        supports: "Observed local evidence supports the claim.".to_string(),
        against: "Contradictory local evidence was considered.".to_string(),
        alternatives: "Alternative synthetic explanations remain bounded.".to_string(),
        data_quality_risks: "The fixture is synthetic.".to_string(),
        assumptions: "The local report format matches the tool contract.".to_string(),
        falsifier: "A contradicting observed link or cap to inferred prevents affirmation."
            .to_string(),
        claim_basis: ClaimBasis::Observed,
        not_yet_claimable: "No external scientific claim is made.".to_string(),
    }
}

#[test]
fn cohort_inferred_marker_param_caps_verified_evidence_and_cannot_affirm() {
    let project = TempProject::new("verified-cap");
    ProjectStore::init(&project.path, Some("Cohort Param Infer Cap")).expect("project should init");
    write_marker_tool(&project.path);
    import_expression_table(&project.path);

    let store = project_store(&project.path);
    let hypothesis = store
        .record_hypothesis(agentflow_core::hypothesis::HypothesisRequest {
            statement: "marker evidence for the target segment requires validation".to_string(),
            origin: "synthetic-test".to_string(),
            related_goal_id: "goal_synthetic_cohort_param".to_string(),
        })
        .expect("hypothesis should record");
    store
        .link_evidence(EvidenceLinkRequest {
            hypothesis_id: hypothesis.id.clone(),
            observation_id: None,
            source: None,
            grade: EvidenceGrade::LiteratureSupported,
            stance: Stance::Supports,
            note: "Synthetic prior support is below the decision margin.".to_string(),
        })
        .expect("weak evidence should link");

    let report = store
        .run_cycle_with_scorer_grounded_cohort(
            ApplyConfig {
                apply: true,
                auto_run: true,
                flow: None,
                max_apply: 5,
                propose_synth: false,
            },
            &NoopParamInferer,
            &NoopRelevanceScorer,
            &NoopOutputGroundingScorer,
            &StubCohortInferer {
                value: "fixture_segment".to_string(),
            },
        )
        .expect("agent run should apply and auto-run");

    let flow_id = format!("auto_{}", hypothesis.id);
    assert_eq!(
        store
            .inferred_params_for_step(&flow_id, "step_marker_emit")
            .expect("inferred params should load"),
        vec![("segment".to_string(), "fixture_segment".to_string())]
    );

    let observation_id = report
        .applied
        .iter()
        .find_map(|action| match action {
            AppliedAction::StepRun {
                observation_id: Some(observation_id),
                ..
            } => Some(observation_id.clone()),
            _ => None,
        })
        .expect("auto-run should produce an observation");
    let linked = store
        .link_evidence(EvidenceLinkRequest {
            hypothesis_id: hypothesis.id.clone(),
            observation_id: Some(observation_id),
            source: None,
            grade: EvidenceGrade::Observed,
            stance: Stance::Supports,
            note: "Verified tool output is capped because the segment param was inferred."
                .to_string(),
        })
        .expect("observed evidence request should link");

    assert_eq!(linked.grade, EvidenceGrade::Inferred);
    let verdict = store
        .render_verdict(&hypothesis.id, &RuleBasedEngine, Some(gate()))
        .expect("verdict should render");
    assert!(matches!(
        verdict.verdict,
        Verdict::Inconclusive(InconclusiveKind::Provisional { .. })
    ));
    assert_ne!(verdict.verdict, Verdict::Affirmed);
}
