//! Regression: when a consumer needs an artifact type that no standalone tool
//! chain can ground, the autonomous loop can expand a registered module as the
//! producer and run the module's internal steps before the consumer.

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
            "agentflow-module-producer-chain-{}-{name}",
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

fn write_passthrough_tool(
    project: &Path,
    namespace: &str,
    name: &str,
    input_port: &str,
    input_type: &str,
    output_port: &str,
    output_type: &str,
) -> PathBuf {
    let script_path = project.join(format!("{namespace}_{name}.sh"));
    let in_env = format!("AGENTFLOW_INPUT_{}", input_port.to_ascii_uppercase());
    let out_env = format!("AGENTFLOW_OUTPUT_{}", output_port.to_ascii_uppercase());
    fs::write(
        &script_path,
        format!("set -eu\ncp \"${in_env}\" \"${out_env}\"\nprintf '{name} ok\\n'\n"),
    )
    .expect("tool script should be written");

    let spec_path = project.join(format!("{namespace}_{name}.tool.yaml"));
    fs::write(
        &spec_path,
        format!(
            r#"
schema_version: agentflow.tool.v0
namespace: {namespace}
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
    .expect("tool spec should be written");
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
  printf 'summary: synthetic module fixture\n'
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

fn write_module(project: &Path) -> PathBuf {
    let module_path = project.join("qc_then_quantify.module.yaml");
    fs::write(
        &module_path,
        r#"
schema_version: agentflow.module.v0
namespace: bio
name: qc_then_quantify
version: 0.1.0
description: QC raw counts then quantify into an expression table.
inputs:
  counts:
    type: RawCounts
outputs:
  expression:
    type: ExpressionTable
    from: quant_out
steps:
  - id: qc
    tool: bio/qc
    inputs:
      counts: counts
    outputs:
      clean: qc_clean
  - id: quant
    tool: bio/quantify
    needs: [qc]
    inputs:
      counts: qc_clean
    outputs:
      expression: quant_out
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
fn agent_backward_chains_registered_module_producer() {
    let project = TempProject::new("module-chain");
    let path = project.path.to_string_lossy().to_string();

    run_agentflow(["init", "--name", "ModuleChain", "--path", &path]);
    let qc = write_passthrough_tool(
        &project.path,
        "bio",
        "qc",
        "counts",
        "ModuleRawCounts",
        "clean",
        "ModuleReadyCounts",
    );
    let quant = write_passthrough_tool(
        &project.path,
        "bio",
        "quantify",
        "counts",
        "ModuleReadyCounts",
        "expression",
        "ExpressionTable",
    );
    let consumer = write_consumer_tool(&project.path);
    let module = write_module(&project.path);
    run_agentflow(["tools", "register", &qc.to_string_lossy(), "--path", &path]);
    run_agentflow([
        "tools",
        "register",
        &quant.to_string_lossy(),
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
        output.contains("__qc") && output.contains("__quant"),
        "module instance steps should appear in agent output\n{output}"
    );
    assert!(
        output.contains("step_marker_emit ran and observed"),
        "consumer step should have run and observed\n{output}"
    );
    assert!(
        !output.contains("missing required"),
        "no required input/param should be missing\n{output}"
    );
}
