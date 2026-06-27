//! Regression: the autonomous loop backward-chains a producer step when the
//! matched consumer needs an input type that is not yet available as an
//! artifact. Given only RawCounts + SurvivalTable and a consumer that needs an
//! ExpressionTable, the agent must draft a producer (RawCounts -> ExpressionTable),
//! wire its output into the consumer, and run both steps in order — without the
//! equivalent-branches brake mistaking the producer for an alternative answer.

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
            "agentflow-multi-step-chain-{}-{name}",
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

fn write_producer_tool(project: &Path) -> PathBuf {
    let script_path = project.join("make_expression.sh");
    fs::write(
        &script_path,
        "set -eu\ncp \"$AGENTFLOW_INPUT_COUNTS\" \"$AGENTFLOW_OUTPUT_EXPRESSION\"\nprintf 'make_expression ok\\n'\n",
    )
    .expect("producer script should be written");

    let spec_path = project.join("make_expression.tool.yaml");
    fs::write(
        &spec_path,
        format!(
            r#"
schema_version: agentflow.tool.v0
namespace: chain
name: make_expression
version: 0.1.0
maturity: wrapped
description: Build an ExpressionTable from RawCounts. Offline fixture producer.
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
    .expect("producer spec should be written");
    spec_path
}

fn write_consumer_tool(project: &Path) -> PathBuf {
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
    .expect("consumer script should be written");

    let spec_path = project.join("marker_emit.tool.yaml");
    fs::write(
        &spec_path,
        format!(
            r#"
schema_version: agentflow.tool.v0
namespace: chain
name: marker_emit
version: 0.1.0
maturity: verified
description: Survival association marker report for a gene over the imported cohort, from a normalized table and a survival table.
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
    .expect("consumer spec should be written");
    spec_path
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
fn agent_backward_chains_producer_for_unavailable_input() {
    let project = TempProject::new("chain");
    let path = project.path.to_string_lossy().to_string();

    run_agentflow(["init", "--name", "ChainTest", "--path", &path]);
    let producer = write_producer_tool(&project.path);
    let consumer = write_consumer_tool(&project.path);
    run_agentflow([
        "tools",
        "register",
        &producer.to_string_lossy(),
        "--path",
        &path,
    ]);
    run_agentflow([
        "tools",
        "register",
        &consumer.to_string_lossy(),
        "--path",
        &path,
    ]);

    let (counts, survival) = write_input_tables(&project.path);
    // Only RawCounts + SurvivalTable are available — no ExpressionTable, so the
    // consumer cannot run without a producer chained in first.
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

    // The producer ran (no observation), then the consumer was patched in and ran.
    assert!(
        output.contains("step_make_expression ran"),
        "producer step should have run\n{output}"
    );
    assert!(
        output.contains("step_marker_emit ran and observed"),
        "consumer step should have run and observed\n{output}"
    );
    // The brake must NOT mistake the producer for an alternative answer, and the
    // consumer must not fail on a missing/unbound ExpressionTable input.
    assert!(
        !output.contains("missing required"),
        "no required input/param should be missing\n{output}"
    );

    // The auto-created flow is a real two-step chain with one edge.
    let flow_id = output
        .lines()
        .find_map(|line| line.trim().strip_prefix("flow "))
        .and_then(|rest| rest.split_whitespace().next())
        .expect("auto-created flow id in output")
        .to_string();
    let inspect = run_agentflow(["flow", "inspect", &flow_id, "--path", &path]);
    assert!(
        inspect.contains("Steps: 2"),
        "flow should have two steps\n{inspect}"
    );
    assert!(
        inspect.contains("Edges: 1"),
        "flow should have one producer->consumer edge\n{inspect}"
    );
}

fn write_passthrough_producer(
    project: &Path,
    name: &str,
    input_port: &str,
    input_type: &str,
    output_port: &str,
    output_type: &str,
) -> PathBuf {
    let script_path = project.join(format!("{name}.sh"));
    let in_env = format!("AGENTFLOW_INPUT_{}", input_port.to_ascii_uppercase());
    let out_env = format!("AGENTFLOW_OUTPUT_{}", output_port.to_ascii_uppercase());
    fs::write(
        &script_path,
        format!("set -eu\ncp \"${in_env}\" \"${out_env}\"\nprintf '{name} ok\\n'\n"),
    )
    .expect("producer script should be written");

    let spec_path = project.join(format!("{name}.tool.yaml"));
    fs::write(
        &spec_path,
        format!(
            r#"
schema_version: agentflow.tool.v0
namespace: chain
name: {name}
version: 0.1.0
maturity: wrapped
description: Offline fixture producer {input_type} to {output_type}.
inputs:
  {input_port}:
    type: {input_type}
    required: true
outputs:
  {output_port}:
    type: {output_type}
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
    .expect("producer spec should be written");
    spec_path
}

/// Two producers must be chained (RawCounts -> MidCounts -> ExpressionTable)
/// because neither the consumer's ExpressionTable nor the middle producer's
/// MidCounts input is available — only RawCounts is. Exercises recursive
/// (multi-level) backward chaining and the transitive equivalent-branch
/// exclusion (neither intermediate producer trips the brake).
#[test]
fn agent_backward_chains_multiple_producer_levels() {
    let project = TempProject::new("multi-level");
    let path = project.path.to_string_lossy().to_string();

    run_agentflow(["init", "--name", "MultiLevel", "--path", &path]);
    let lower = write_passthrough_producer(
        &project.path,
        "lower",
        "counts",
        "RawCounts",
        "mid",
        "MidCounts",
    );
    let upper = write_passthrough_producer(
        &project.path,
        "upper",
        "mid",
        "MidCounts",
        "expression",
        "ExpressionTable",
    );
    let consumer = write_consumer_tool(&project.path);
    run_agentflow([
        "tools",
        "register",
        &lower.to_string_lossy(),
        "--path",
        &path,
    ]);
    run_agentflow([
        "tools",
        "register",
        &upper.to_string_lossy(),
        "--path",
        &path,
    ]);
    run_agentflow([
        "tools",
        "register",
        &consumer.to_string_lossy(),
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
        output.contains("step_lower ran") && output.contains("step_upper ran"),
        "both chained producers should have run\n{output}"
    );
    assert!(
        output.contains("step_marker_emit ran and observed"),
        "consumer step should have run and observed\n{output}"
    );
    assert!(
        !output.contains("missing required"),
        "no required input/param should be missing\n{output}"
    );

    let flow_id = output
        .lines()
        .find_map(|line| line.trim().strip_prefix("flow "))
        .and_then(|rest| rest.split_whitespace().next())
        .expect("auto-created flow id in output")
        .to_string();
    let inspect = run_agentflow(["flow", "inspect", &flow_id, "--path", &path]);
    assert!(
        inspect.contains("Steps: 3"),
        "flow should have three chained steps\n{inspect}"
    );
    assert!(
        inspect.contains("Edges: 2"),
        "flow should have two chain edges\n{inspect}"
    );
}

#[test]
fn agent_max_chain_depth_bounds_producer_chaining() {
    // The same two-level ladder (RawCounts -> MidCounts -> ExpressionTable), but
    // `--max-chain-depth 1` only allows a one-level chain — the upper producer
    // itself needs a second level (MidCounts), so the chain cannot form and the
    // consumer's ExpressionTable input is left unsatisfied.
    let project = TempProject::new("chain-depth");
    let path = project.path.to_string_lossy().to_string();

    run_agentflow(["init", "--name", "ChainDepth", "--path", &path]);
    let lower = write_passthrough_producer(
        &project.path,
        "lower",
        "counts",
        "RawCounts",
        "mid",
        "MidCounts",
    );
    let upper = write_passthrough_producer(
        &project.path,
        "upper",
        "mid",
        "MidCounts",
        "expression",
        "ExpressionTable",
    );
    let consumer = write_consumer_tool(&project.path);
    run_agentflow([
        "tools",
        "register",
        &lower.to_string_lossy(),
        "--path",
        &path,
    ]);
    run_agentflow([
        "tools",
        "register",
        &upper.to_string_lossy(),
        "--path",
        &path,
    ]);
    run_agentflow([
        "tools",
        "register",
        &consumer.to_string_lossy(),
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
        "--max-chain-depth",
        "1",
        "--no-auto-synth",
        "--no-auto-forage",
        "--no-semantic-match",
        "--path",
        &path,
    ]);

    // With depth 1 the two-level chain does not form, so neither the second-level
    // producer nor the consumer (whose ExpressionTable stays unsatisfied) runs —
    // unlike the default-depth run above, which produces a 3-step flow.
    assert!(
        !output.contains("step_upper ran"),
        "depth 1 must not reach the second-level producer\n{output}"
    );
    assert!(
        !output.contains("step_marker_emit ran"),
        "the consumer cannot run without its chained input at depth 1\n{output}"
    );
}
