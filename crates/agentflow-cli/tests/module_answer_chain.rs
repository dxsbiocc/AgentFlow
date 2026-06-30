//! Regression: when no standalone tool path can answer a hypothesis, the
//! autonomous loop can expand a registered module and use its observed internal
//! step as the answer.

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
            "agentflow-module-answer-chain-{}-{name}",
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

fn write_prep_tool(project: &Path) -> PathBuf {
    let script_path = project.join("bio_prep.sh");
    fs::write(
        &script_path,
        "set -eu\ncp \"$AGENTFLOW_INPUT_COUNTS\" \"$AGENTFLOW_OUTPUT_EXPRESSION\"\nprintf 'prep ok\\n'\n",
    )
    .expect("prep script should be written");

    let spec_path = project.join("bio_prep.tool.yaml");
    fs::write(
        &spec_path,
        format!(
            r#"
schema_version: agentflow.tool.v0
namespace: bio
name: prep
version: 0.1.0
maturity: wrapped
description: Prepare raw counts as an expression table.
inputs:
  counts:
    type: RawCounts
    required: true
outputs:
  expression:
    type: ExpressionTable
    min_rows: 1
runtime:
  backend: local
  command:
    - /bin/sh
    - {}
"#,
            script_path.display()
        ),
    )
    .expect("prep spec should be written");
    spec_path
}

fn write_report_tool(project: &Path) -> PathBuf {
    let script_path = project.join("bio_report.sh");
    fs::write(
        &script_path,
        r#"set -eu
cat "$AGENTFLOW_INPUT_EXPRESSION_TABLE" >/dev/null
cat "$AGENTFLOW_INPUT_SURVIVAL_TABLE" >/dev/null
marker="${AGENTFLOW_PARAM_MARKER:?missing marker}"
{
  printf '# Marker report\n'
  printf 'Gene: %s\n' "$marker"
  printf 'score: 0.91\n'
  printf 'rows: 4\n'
  printf 'summary: synthetic module answer fixture\n'
} > "$AGENTFLOW_OUTPUT_REPORT"
printf 'report ok\n'
"#,
    )
    .expect("report script should be written");

    let spec_path = project.join("bio_report.tool.yaml");
    fs::write(
        &spec_path,
        format!(
            r#"
schema_version: agentflow.tool.v0
namespace: bio
name: report
version: 0.1.0
maturity: verified
description: Emit a module-scoped survival association report.
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
    .expect("report spec should be written");
    spec_path
}

fn write_module(project: &Path) -> PathBuf {
    let module_path = project.join("bio_assoc_report.module.yaml");
    fs::write(
        &module_path,
        r#"
schema_version: agentflow.module.v0
namespace: bio
name: assoc_report
version: 0.1.0
description: Prepare raw counts and emit a fixed-parameter association report.
inputs:
  counts:
    type: RawCounts
  survival:
    type: SurvivalTable
outputs:
  report:
    type: Markdown
    from: report_md
steps:
  - id: prep
    tool: bio/prep
    inputs:
      counts: counts
    outputs:
      expression: expression_table
  - id: report
    tool: bio/report
    needs: [prep]
    inputs:
      expression_table: expression_table
      survival_table: survival
    params:
      marker: MODULE_FIXED_MARKER
    outputs:
      report: report_md
"#,
    )
    .expect("module spec should be written");
    module_path
}

fn write_input_tables(project: &Path) -> (PathBuf, PathBuf) {
    let counts_path = project.join("counts.tsv");
    fs::write(&counts_path, "sample\tM1\ns1\t1\ns2\t2\ns3\t8\ns4\t9\n")
        .expect("counts table should be written");

    let survival_path = project.join("survival.tsv");
    fs::write(
        &survival_path,
        "sample\ttime\tstatus\ns1\t4\t1\ns2\t6\t1\ns3\t8\t0\ns4\t10\t0\n",
    )
    .expect("survival table should be written");

    (counts_path, survival_path)
}

#[test]
fn agent_answers_hypothesis_with_registered_module() {
    let project = TempProject::new("module-answer");
    let path = project.path.to_string_lossy().to_string();

    run_agentflow(["init", "--name", "ModuleAnswer", "--path", &path]);
    let prep = write_prep_tool(&project.path);
    let report = write_report_tool(&project.path);
    let module = write_module(&project.path);
    run_agentflow([
        "tools",
        "register",
        &prep.to_string_lossy(),
        "--path",
        &path,
    ]);
    run_agentflow([
        "tools",
        "register",
        &report.to_string_lossy(),
        "--path",
        &path,
    ]);
    run_agentflow([
        "module",
        "register",
        &module.to_string_lossy(),
        "--path",
        &path,
    ]);

    let (counts, survival) = write_input_tables(&project.path);
    run_agentflow([
        "import",
        &counts.to_string_lossy(),
        "--type",
        "RawCounts",
        "--mode",
        "copy",
        "--path",
        &path,
    ]);
    run_agentflow([
        "import",
        &survival.to_string_lossy(),
        "--type",
        "SurvivalTable",
        "--mode",
        "copy",
        "--path",
        &path,
    ]);
    run_agentflow([
        "hypothesis",
        "create",
        "--statement",
        "M1 shows a survival association in the imported cohort",
        "--origin",
        "user_goal",
        "--goal",
        "g1",
        "--path",
        &path,
    ]);

    let output = run_agentflow([
        "agent",
        "run",
        "--apply",
        "--auto-run",
        "--no-auto-synth",
        "--no-auto-forage",
        "--no-semantic-match",
        "--path",
        &path,
    ]);

    assert!(
        output.contains("matched tool: bio/report (high)"),
        "module answer proposal should expose the real answer tool ref\n{output}"
    );
    // The instance prefix carries the module ref (sanitized), so these assert the
    // steps ran as part of the `bio/assoc_report` MODULE instance — a standalone
    // `bio/report` tool path could not produce a `__bio_assoc_report__` prefix.
    assert!(
        output.contains("__bio_assoc_report__prep ran without observation"),
        "module prep step should have run as a module instance\n{output}"
    );
    assert!(
        output.contains("__bio_assoc_report__report ran and observed"),
        "module report step should have run and observed as a module instance\n{output}"
    );
    assert!(
        !output.contains("step step_prep ran"),
        "standalone producer step should not be the applied answer path\n{output}"
    );
    assert!(
        !output.contains("missing required"),
        "no required input/param should be missing\n{output}"
    );

    let observations = run_agentflow(["observations", "list", "--json", "--path", &path]);
    assert!(
        observations.contains("\"kind\":\"marker_report\""),
        "module report should produce a marker observation\n{observations}"
    );
    assert!(
        observations.contains("MODULE_FIXED_MARKER"),
        "module step should supply the fixed report param\n{observations}"
    );
}

/// Like `write_report_tool` but the `marker` param is inferable (`infer: gene`),
/// so the agent can fill it from the hypothesis when the module leaves it unset.
fn write_report_tool_inferred(project: &Path) -> PathBuf {
    let script_path = project.join("bio_report_inferred.sh");
    fs::write(
        &script_path,
        r#"set -eu
cat "$AGENTFLOW_INPUT_EXPRESSION_TABLE" >/dev/null
cat "$AGENTFLOW_INPUT_SURVIVAL_TABLE" >/dev/null
marker="${AGENTFLOW_PARAM_MARKER:?missing marker}"
cohort="${AGENTFLOW_PARAM_COHORT:?missing cohort}"
{
  printf '# Marker report\n'
  printf 'Gene: %s\n' "$marker"
  printf 'Cohort: %s\n' "$cohort"
  printf 'score: 0.91\n'
  printf 'rows: 4\n'
  printf 'summary: inferred-marker module answer fixture\n'
} > "$AGENTFLOW_OUTPUT_REPORT"
printf 'report ok\n'
"#,
    )
    .expect("report script should be written");

    let spec_path = project.join("bio_report_inferred.tool.yaml");
    fs::write(
        &spec_path,
        format!(
            r#"
schema_version: agentflow.tool.v0
namespace: bio
name: report
version: 0.1.0
maturity: verified
description: Emit a module-scoped survival association report for a gene.
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
    infer: gene
  cohort:
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
    .expect("report tool spec should be written");
    spec_path
}

/// Like `write_module` but the report step OMITS the `marker` param, so the agent
/// must infer it from the hypothesis (slice 4b-4c).
fn write_module_inferred(project: &Path) -> PathBuf {
    let module_path = project.join("bio_assoc_report.module.yaml");
    fs::write(
        &module_path,
        r#"
schema_version: agentflow.module.v0
namespace: bio
name: assoc_report
version: 0.1.0
description: Prepare raw counts and emit an association report for the inferred gene.
inputs:
  counts:
    type: RawCounts
  survival:
    type: SurvivalTable
outputs:
  report:
    type: Markdown
    from: report_md
steps:
  - id: prep
    tool: bio/prep
    inputs:
      counts: counts
    outputs:
      expression: expression_table
  - id: report
    tool: bio/report
    needs: [prep]
    inputs:
      expression_table: expression_table
      survival_table: survival
    params:
      cohort: FIXED_COHORT
    outputs:
      report: report_md
"#,
    )
    .expect("module spec should be written");
    module_path
}

#[test]
fn agent_infers_module_answer_param_from_hypothesis() {
    let project = TempProject::new("module-answer-infer");
    let path = project.path.to_string_lossy().to_string();

    run_agentflow(["init", "--name", "ModuleAnswerInfer", "--path", &path]);
    let prep = write_prep_tool(&project.path);
    let report = write_report_tool_inferred(&project.path);
    let module = write_module_inferred(&project.path);
    run_agentflow([
        "tools",
        "register",
        &prep.to_string_lossy(),
        "--path",
        &path,
    ]);
    run_agentflow([
        "tools",
        "register",
        &report.to_string_lossy(),
        "--path",
        &path,
    ]);
    run_agentflow([
        "module",
        "register",
        &module.to_string_lossy(),
        "--path",
        &path,
    ]);

    let (counts, survival) = write_input_tables(&project.path);
    run_agentflow([
        "import",
        &counts.to_string_lossy(),
        "--type",
        "RawCounts",
        "--mode",
        "copy",
        "--path",
        &path,
    ]);
    run_agentflow([
        "import",
        &survival.to_string_lossy(),
        "--type",
        "SurvivalTable",
        "--mode",
        "copy",
        "--path",
        &path,
    ]);
    // The hypothesis names a gene; the module omits the marker, so the agent must
    // infer TP53 from the statement and fill the answer step's param.
    run_agentflow([
        "hypothesis",
        "create",
        "--statement",
        "TP53 shows a survival association in the imported cohort",
        "--origin",
        "user_goal",
        "--goal",
        "g1",
        "--path",
        &path,
    ]);

    let output = run_agentflow([
        "agent",
        "run",
        "--apply",
        "--auto-run",
        "--no-auto-synth",
        "--no-auto-forage",
        "--no-semantic-match",
        "--path",
        &path,
    ]);
    assert!(
        output.contains("__bio_assoc_report__report ran and observed"),
        "module answer step should have run as a module instance\n{output}"
    );
    assert!(
        !output.contains("missing required"),
        "the inferred marker should satisfy the answer step's required param\n{output}"
    );
    // Honesty interlock: the inferred param is recorded as an unconfirmed inferred
    // value, so the verdict stays grade-capped (it cannot autonomously affirm).
    assert!(
        output.contains("marker=TP53"),
        "the inferred marker must be recorded as an unconfirmed inferred param\n{output}"
    );

    let observations = run_agentflow(["observations", "list", "--json", "--path", &path]);
    assert!(
        observations.contains("\"kind\":\"marker_report\""),
        "module report should produce a marker observation\n{observations}"
    );
    // The gene inferred from the hypothesis (not a fixed module value) reached the
    // answer step's report.
    assert!(
        observations.contains("TP53"),
        "the agent should have inferred TP53 from the hypothesis into the module answer\n{observations}"
    );
}
