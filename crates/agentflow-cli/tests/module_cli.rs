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
            "agentflow-module-cli-{}-{name}",
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

#[test]
fn module_validate_command_accepts_valid_yaml() {
    let project = TempProject::new("validate");
    let module_path = write_module_yaml(&project.path);
    let module_arg = module_path.to_str().expect("module path should be UTF-8");

    let output = run_agentflow(["module", "validate", module_arg]);

    assert!(output.contains("Module bio/qc_then_quantify is valid"));
    assert!(output.contains("Version: 0.1.0"));
    assert!(output.contains("Steps: 2"));
    assert!(output.contains("Inputs: 1"));
    assert!(output.contains("Outputs: 1"));
}

#[test]
fn module_validate_command_rejects_invalid_yaml() {
    let project = TempProject::new("validate-bad");
    // An output port that maps to an artifact no step produces — rejected by
    // ModuleSpec::from_simple_yaml's validation.
    let module_path = project.path.join("bad.module.yaml");
    fs::write(
        &module_path,
        r#"
schema_version: agentflow.module.v0
name: bad
version: 0.1.0
description: output maps to nothing
inputs:
  counts:
    type: RawCounts
outputs:
  result:
    type: ExpressionTable
    from: nonexistent
steps:
  - id: only
    tool: bio/qc
    inputs:
      counts: counts
    outputs:
      clean: qc_clean
"#,
    )
    .expect("module spec should be written");
    let module_arg = module_path.to_str().expect("module path should be UTF-8");

    let (success, _stdout, stderr) = try_agentflow(["module", "validate", module_arg]);
    assert!(!success, "invalid module should make the CLI exit non-zero");
    assert!(
        stderr.contains("which no step produces"),
        "stderr should explain the validation failure: {stderr}"
    );
}

#[test]
fn module_show_command_renders_ports_and_steps() {
    let project = TempProject::new("show");
    let module_path = write_module_yaml(&project.path);
    let module_arg = module_path.to_str().expect("module path should be UTF-8");

    let output = run_agentflow(["module", "show", module_arg]);

    assert!(output.contains("Module: bio/qc_then_quantify"));
    assert!(output.contains("counts: RawCounts"));
    assert!(output.contains("expression: ExpressionTable <- quant_out"));
    assert!(output.contains("qc: bio/qc"));
    assert!(output.contains("quant: bio/quantify (needs: qc)"));
}
