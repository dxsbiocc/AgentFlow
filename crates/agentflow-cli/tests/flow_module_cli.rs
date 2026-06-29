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
            "agentflow-flow-module-cli-{}-{name}",
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

/// Run the CLI without asserting success; returns (success, stdout, stderr).
fn try_agentflow<I, S>(args: I) -> (bool, String, String)
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = agentflow_command()
        .args(args)
        .output()
        .expect("agentflow CLI should execute");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn write_module_yaml(project: &Path) -> PathBuf {
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

fn write_flow_yaml(project: &Path) -> PathBuf {
    let flow_path = project.join("module_flow.yaml");
    fs::write(
        &flow_path,
        r#"
schema_version: agentflow.flow.v0
id: flow_with_module
name: Flow with module
steps:
  - id: prep
    module: bio/qc_then_quantify
    inputs:
      counts: artifact_raw
"#,
    )
    .expect("flow spec should be written");
    flow_path
}

#[test]
fn flow_module_flag_supplies_module_specs_to_validate() {
    let project = TempProject::new("validate");
    let module_path = write_module_yaml(&project.path);
    let flow_path = write_flow_yaml(&project.path);
    let project_arg = project.path.to_str().expect("project path should be UTF-8");
    let module_arg = module_path.to_str().expect("module path should be UTF-8");
    let flow_arg = flow_path.to_str().expect("flow path should be UTF-8");

    run_agentflow(["init", "--name", "flow-module-test", "--path", project_arg]);

    let (missing_success, missing_stdout, missing_stderr) =
        try_agentflow(["flow", "validate", flow_arg, "--path", project_arg]);
    assert!(
        !missing_success,
        "flow validate without --module should fail\nstdout:\n{missing_stdout}\nstderr:\n{missing_stderr}"
    );
    assert!(
        missing_stderr.contains("module bio/qc_then_quantify")
            && missing_stderr.contains("which was not provided"),
        "stderr should report the missing module\nstdout:\n{missing_stdout}\nstderr:\n{missing_stderr}"
    );

    let (success, stdout, stderr) = try_agentflow([
        "flow",
        "validate",
        flow_arg,
        "--module",
        module_arg,
        "--path",
        project_arg,
    ]);
    if success {
        assert!(
            stdout.contains("Flow is valid"),
            "successful validation should report a valid flow\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
    } else {
        assert!(
            stderr.contains("flow validation failed"),
            "--module should be accepted and reach flow validation\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            !stderr.contains("which was not provided"),
            "--module should remove the missing-module error\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
    }
}
