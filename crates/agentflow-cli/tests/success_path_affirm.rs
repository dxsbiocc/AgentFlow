use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct TempProject {
    path: PathBuf,
}

impl TempProject {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "agentflow-success-path-affirm-{}-{name}",
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

fn agentflow_command() -> Command {
    if let Some(path) = option_env!("CARGO_BIN_EXE_agentflow") {
        Command::new(path)
    } else {
        let mut command = Command::new("cargo");
        command.args(["run", "-q", "-p", "agentflow-cli", "--"]);
        command
    }
}

fn run_agentflow<I, S>(args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = agentflow_command()
        .args(args)
        .output()
        .expect("agentflow CLI should execute");
    assert!(
        output.status.success(),
        "agentflow CLI failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("agentflow stdout should be UTF-8")
}

fn extract_after_line(output: &str, prefix: &str) -> String {
    output
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .expect("expected prefixed line in CLI output")
        .to_string()
}

fn extract_json_field(output: &str, field: &str) -> String {
    let marker = format!("\"{field}\":\"");
    output
        .split(&marker)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("expected JSON field in CLI output")
        .to_string()
}

fn first_observation_id(output: &str) -> String {
    output
        .split("\"observations\":[{\"id\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("expected at least one observation")
        .to_string()
}

fn write_marker_emit_tool(project: &Path, maturity: &str) -> PathBuf {
    let script_path = project.join("marker_emit.sh");
    fs::write(
        &script_path,
        r#"set -eu
cat "$AGENTFLOW_INPUT_EXPRESSION_TABLE" >/dev/null
cat "$AGENTFLOW_INPUT_SURVIVAL_TABLE" >/dev/null
marker="${AGENTFLOW_PARAM_MARKER:?missing marker}"
{
  printf '# Marker report\n'
  printf 'Marker: %s\n' "$marker"
  printf 'score: 0.90\n'
  printf 'rows: 4\n'
  printf 'summary: synthetic fixture\n'
} > "$AGENTFLOW_OUTPUT_REPORT"
printf 'marker_emit ok\n'
"#,
    )
    .expect("tool script should be written");

    let spec_path = project.join(format!("marker_emit_{maturity}.tool.yaml"));
    fs::write(
        &spec_path,
        format!(
            r#"
schema_version: agentflow.tool.v0
namespace: synthetic
name: marker_emit
version: 0.1.0
maturity: {maturity}
description: Emit a deterministic synthetic marker report from two local tables.
inputs:
  expression_table:
    type: ExpressionTable
    required: true
  survival_table:
    type: SurvivalTable
    required: true
params:
  marker:
    type: string
    required: true
outputs:
  report:
    type: Markdown
    observer: marker_report
    min_rows: 4
runtime:
  backend: local
  command:
    - /bin/sh
    - {}
"#,
            script_path.display()
        ),
    )
    .expect("tool spec should be written");
    spec_path
}

fn write_input_tables(project: &Path) -> (PathBuf, PathBuf) {
    let expression_path = project.join("expression.tsv");
    fs::write(
        &expression_path,
        "\
sample\tM1
s1\t0.10
s2\t0.20
s3\t0.80
s4\t0.90
",
    )
    .expect("expression table should be written");

    let survival_path = project.join("survival.tsv");
    fs::write(
        &survival_path,
        "\
sample\ttime\tstatus
s1\t4\t1
s2\t6\t1
s3\t8\t0
s4\t10\t0
",
    )
    .expect("survival table should be written");

    (expression_path, survival_path)
}

fn write_flow(project: &Path, flow_id: &str, expression_id: &str, survival_id: &str) -> PathBuf {
    let flow_path = project.join(format!("{flow_id}.flow.yaml"));
    fs::write(
        &flow_path,
        format!(
            r#"
schema_version: agentflow.flow.v0
id: {flow_id}
name: Synthetic marker emission
steps:
  - id: emit
    tool: synthetic/marker_emit
    reason: Emit a fixed local marker report from imported synthetic tables.
    needs: []
    inputs:
      expression_table: {expression_id}
      survival_table: {survival_id}
    params:
      marker: M1
    outputs:
      report: marker_report
"#
        ),
    )
    .expect("flow should be written");
    flow_path
}

fn create_scenario(maturity: &str, name: &str) -> (TempProject, String, String) {
    let project = TempProject::new(name);
    let project_arg = project.path.to_str().expect("temp path should be UTF-8");

    run_agentflow([
        "init",
        "--name",
        "Synthetic Success Path",
        "--path",
        project_arg,
    ]);

    let tool_path = write_marker_emit_tool(&project.path, maturity);
    run_agentflow([
        "tools",
        "register",
        tool_path.to_str().expect("tool path should be UTF-8"),
        "--path",
        project_arg,
    ]);

    let (expression_path, survival_path) = write_input_tables(&project.path);
    let expression_import = run_agentflow([
        "import",
        expression_path
            .to_str()
            .expect("expression path should be UTF-8"),
        "--type",
        "ExpressionTable",
        "--path",
        project_arg,
    ]);
    let expression_id = extract_after_line(&expression_import, "Id: ");

    let survival_import = run_agentflow([
        "import",
        survival_path
            .to_str()
            .expect("survival path should be UTF-8"),
        "--type",
        "SurvivalTable",
        "--path",
        project_arg,
    ]);
    let survival_id = extract_after_line(&survival_import, "Id: ");

    let hypothesis = run_agentflow([
        "hypothesis",
        "create",
        "--statement",
        "marker M1 associates with outcome in the imported synthetic cohort",
        "--origin",
        "synthetic-test",
        "--goal",
        "goal_synthetic_success_path",
        "--json",
        "--path",
        project_arg,
    ]);
    let hypothesis_id = extract_json_field(&hypothesis, "id");

    let flow_id = format!("synthetic_{maturity}_{name}");
    let flow_path = write_flow(&project.path, &flow_id, &expression_id, &survival_id);
    let validation = run_agentflow([
        "flow",
        "validate",
        flow_path.to_str().expect("flow path should be UTF-8"),
        "--json",
        "--path",
        project_arg,
    ]);
    assert!(validation.contains("\"valid\":true"), "{validation}");

    run_agentflow([
        "flow",
        "approve",
        flow_path.to_str().expect("flow path should be UTF-8"),
        "--path",
        project_arg,
    ]);

    let run = run_agentflow(["run", &flow_id, "--path", project_arg]);
    assert!(run.contains("Completed steps: 1"), "{run}");
    assert!(run.contains("Failed steps: 0"), "{run}");

    let observations = run_agentflow(["observations", "list", "--json", "--path", project_arg]);
    assert!(
        observations.contains("\"kind\":\"marker_report\""),
        "{observations}"
    );
    assert!(observations.contains("\"flow_id\":\""), "{observations}");
    assert!(observations.contains("\"step_id\":\""), "{observations}");
    let observation_id = first_observation_id(&observations);

    (project, hypothesis_id, observation_id)
}

fn link_supporting_observed(project: &Path, hypothesis_id: &str, observation_id: &str) -> String {
    run_agentflow([
        "evidence",
        "link",
        "--hypothesis",
        hypothesis_id,
        "--observation",
        observation_id,
        "--grade",
        "observed",
        "--stance",
        "supports",
        "--note",
        "Synthetic marker report supports this local mechanism check.",
        "--json",
        "--path",
        project.to_str().expect("project path should be UTF-8"),
    ])
}

fn render_verdict(project: &Path, hypothesis_id: &str) -> String {
    run_agentflow([
        "verdict",
        "render",
        "--hypothesis",
        hypothesis_id,
        "--gate-supports",
        "Observed local evidence supports the claim.",
        "--gate-against",
        "Contradictory local evidence was considered for this fixture.",
        "--gate-alternatives",
        "Alternative synthetic explanations remain bounded to the fixture.",
        "--gate-data-risks",
        "The fixture is synthetic and only checks the success-path mechanism.",
        "--gate-assumptions",
        "The local tables and report format match declared tool contracts.",
        "--gate-falsifier",
        "A contradicting observed link or cap to inferred would prevent affirmation.",
        "--gate-claim-basis",
        "observed",
        "--gate-not-yet",
        "No external scientific claim is made.",
        "--json",
        "--path",
        project.to_str().expect("project path should be UTF-8"),
    ])
}

fn show_verdict(project: &Path, hypothesis_id: &str) -> String {
    run_agentflow([
        "verdict",
        "show",
        "--hypothesis",
        hypothesis_id,
        "--json",
        "--path",
        project.to_str().expect("project path should be UTF-8"),
    ])
}

fn evidence_list(project: &Path, hypothesis_id: &str) -> String {
    run_agentflow([
        "evidence",
        "list",
        "--hypothesis",
        hypothesis_id,
        "--json",
        "--path",
        project.to_str().expect("project path should be UTF-8"),
    ])
}

#[test]
fn verified_tool_with_confirmed_marker_keeps_observed_evidence_and_affirms() {
    let (project, hypothesis_id, observation_id) = create_scenario("verified", "positive");

    let linked = link_supporting_observed(&project.path, &hypothesis_id, &observation_id);
    assert!(linked.contains("\"grade\":\"observed\""), "{linked}");

    let evidence = evidence_list(&project.path, &hypothesis_id);
    assert!(evidence.contains("\"grade\":\"observed\""), "{evidence}");
    assert!(!evidence.contains("\"grade\":\"inferred\""), "{evidence}");

    let verdict = render_verdict(&project.path, &hypothesis_id);
    assert!(verdict.contains("\"verdict\":\"affirmed\""), "{verdict}");

    let shown = show_verdict(&project.path, &hypothesis_id);
    assert!(shown.contains("\"tag\":\"affirmed\""), "{shown}");
}

#[test]
fn exploratory_tool_caps_observed_evidence_and_cannot_affirm() {
    let (project, hypothesis_id, observation_id) = create_scenario("exploratory", "negative");

    let linked = link_supporting_observed(&project.path, &hypothesis_id, &observation_id);
    assert!(linked.contains("\"grade\":\"inferred\""), "{linked}");
    assert!(!linked.contains("\"grade\":\"observed\""), "{linked}");

    let evidence = evidence_list(&project.path, &hypothesis_id);
    assert!(evidence.contains("\"grade\":\"inferred\""), "{evidence}");
    assert!(!evidence.contains("\"grade\":\"observed\""), "{evidence}");

    let verdict = render_verdict(&project.path, &hypothesis_id);
    assert!(!verdict.contains("\"verdict\":\"affirmed\""), "{verdict}");
    assert!(
        verdict.contains("\"verdict\":\"inconclusive\""),
        "{verdict}"
    );

    let shown = show_verdict(&project.path, &hypothesis_id);
    assert!(!shown.contains("\"tag\":\"affirmed\""), "{shown}");
    assert!(
        shown.contains("\"tag\":\"inconclusive_provisional\""),
        "{shown}"
    );
}

// The inferred-parameter cap is already covered in core unit tests. Constructing it
// through the CLI requires autonomous parameter inference bookkeeping, which this
// regression intentionally avoids so it never invokes an LLM or network path.
