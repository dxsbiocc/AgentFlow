use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agentflow_core::domain::ToolMaturity;
use agentflow_core::storage::{ProjectStore, ToolSpec};

use crate::{next_arg, require_value, CliError};

pub(crate) const DEFAULT_SYNTHESIZER: &str = "claude -p";
const SYNTH_VERSION: &str = "0.1.0";
const VALIDATION_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Default)]
struct SynthOptions {
    name: Option<String>,
    description: Option<String>,
    fixture: Option<PathBuf>,
    expect: Option<String>,
    synthesizer: Option<String>,
    path: Option<PathBuf>,
}

#[derive(Debug)]
struct ValidationOutput {
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
    timed_out: bool,
}

pub(crate) fn synth_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_synth_options(args)?;
    run_synth(options)
}

fn run_synth(options: SynthOptions) -> Result<String, CliError> {
    let name = require_option(options.name, "--name")?;
    validate_tool_name(&name)?;
    let description = require_option(options.description, "--description")?;
    let fixture = require_option(options.fixture, "--fixture")?;
    let fixture = fs::canonicalize(&fixture).map_err(|error| {
        CliError::Core(format!(
            "failed to resolve fixture {}: {error}",
            fixture.display()
        ))
    })?;
    let expect = require_option(options.expect, "--expect")?;
    let project_path = options.path.unwrap_or(std::env::current_dir()?);
    let store = ProjectStore::open(&project_path)?;
    let script_path = synth_script_path(store.root_path(), &name);

    let prompt = build_synth_prompt(&description);
    let synthesizer = options
        .synthesizer
        .unwrap_or_else(|| DEFAULT_SYNTHESIZER.to_string());
    let candidate = run_synthesizer(&synthesizer, &prompt)?;
    let script = strip_markdown_fence(&candidate);
    if script.trim().is_empty() {
        return Err(CliError::Core(
            "synthesizer produced an empty candidate script".to_string(),
        ));
    }

    if let Some(parent) = script_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&script_path, script.as_bytes())?;
    let script_path = fs::canonicalize(&script_path)?;

    let validation = validate_candidate_script(&script_path, &fixture)?;
    if validation.timed_out {
        return Err(CliError::Core(format!(
            "candidate script timed out after {}s\nScript: {}\nStdout:\n{}\nStderr:\n{}",
            VALIDATION_TIMEOUT.as_secs(),
            script_path.display(),
            validation.stdout,
            validation.stderr
        )));
    }
    if validation.exit_code != Some(0) {
        return Err(CliError::Core(format!(
            "candidate script failed with exit code {}\nScript: {}\nStdout:\n{}\nStderr:\n{}",
            validation
                .exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            script_path.display(),
            validation.stdout,
            validation.stderr
        )));
    }
    if !validation.stdout.contains(&expect) {
        return Ok(format!(
            concat!(
                "REJECTED\n",
                "Script: {}\n",
                "Expected substring: {}\n",
                "Stdout:\n{}\n",
                "Stderr:\n{}"
            ),
            script_path.display(),
            expect,
            validation.stdout,
            validation.stderr
        ));
    }

    let spec_yaml = synthesized_tool_yaml(&name, &description, &script_path);
    let spec = ToolSpec::from_simple_yaml(&spec_yaml)?;
    let registration = store.register_tool(spec)?;
    Ok(format!(
        concat!(
            "VALIDATED -> registered as exploratory (low trust)\n",
            "Tool: {}\n",
            "Version: {}\n",
            "Script: {}\n",
            "Spec hash: {}"
        ),
        registration.tool_ref,
        registration.version,
        script_path.display(),
        registration.spec_hash
    ))
}

fn parse_synth_options<I>(args: I) -> Result<SynthOptions, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = SynthOptions::default();
    let mut args = args.into_iter();

    while let Some(arg) = next_arg(&mut args)? {
        match arg.as_str() {
            "--name" => options.name = Some(require_value("--name", &mut args)?),
            "--description" => {
                options.description = Some(require_value("--description", &mut args)?);
            }
            "--fixture" => {
                options.fixture = Some(PathBuf::from(require_value("--fixture", &mut args)?));
            }
            "--expect" => options.expect = Some(require_value("--expect", &mut args)?),
            "--synthesizer" => {
                options.synthesizer = Some(require_value("--synthesizer", &mut args)?);
            }
            "--path" => options.path = Some(PathBuf::from(require_value("--path", &mut args)?)),
            _ if arg.starts_with('-') => {
                return Err(CliError::InvalidArgument(format!("unknown option: {arg}")));
            }
            _ => {
                return Err(CliError::InvalidArgument(format!(
                    "synth does not accept positional arguments: {arg}"
                )));
            }
        }
    }

    Ok(options)
}

fn require_option<T>(value: Option<T>, flag: &str) -> Result<T, CliError> {
    value.ok_or_else(|| CliError::InvalidArgument(format!("synth requires {flag}")))
}

fn validate_tool_name(name: &str) -> Result<(), CliError> {
    if name.trim().is_empty() {
        return Err(CliError::InvalidArgument(
            "--name must not be empty".to_string(),
        ));
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err(CliError::InvalidArgument(
            "--name may only contain ASCII letters, numbers, underscore, dash, and dot".to_string(),
        ));
    }
    Ok(())
}

fn build_synth_prompt(description: &str) -> String {
    format!(
        concat!(
            "Write a self-contained Python 3 script using only the Python standard library.\n",
            "The script must read the input file path from the SYNTH_INPUT environment variable.\n",
            "The script must write its result to stdout.\n",
            "Task description:\n",
            "{}\n\n",
            "Return only raw Python code. Do not include markdown fences, explanations, or comments outside the code."
        ),
        description
    )
}

pub(crate) fn run_synthesizer(command_line: &str, prompt: &str) -> Result<String, CliError> {
    let argv = split_synthesizer_command(command_line)?;
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]).arg(prompt);
    let output = command.output().map_err(|error| {
        CliError::Core(format!(
            "failed to run synthesizer `{command_line}`: {error}"
        ))
    })?;
    if !output.status.success() {
        return Err(CliError::Core(format!(
            "synthesizer failed with status {}: {}",
            format_exit_status(&output.status),
            stderr_summary(&output.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub(crate) fn split_synthesizer_command(command_line: &str) -> Result<Vec<String>, CliError> {
    let argv = command_line
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if argv.is_empty() {
        return Err(CliError::InvalidArgument(
            "--synthesizer must not be empty".to_string(),
        ));
    }
    Ok(argv)
}

pub(crate) fn strip_markdown_fence(candidate: &str) -> String {
    let trimmed = candidate.trim();
    let mut lines = trimmed.lines().collect::<Vec<_>>();
    if lines
        .first()
        .is_some_and(|line| line.trim_start().starts_with("```"))
    {
        lines.remove(0);
        if lines
            .last()
            .is_some_and(|line| line.trim_start().starts_with("```"))
        {
            lines.pop();
        }
        return lines.join("\n").trim().to_string();
    }
    trimmed.to_string()
}

fn synth_script_path(project_root: &Path, name: &str) -> PathBuf {
    project_root
        .join(".agentflow")
        .join("synth")
        .join(format!("{name}.py"))
}

fn validate_candidate_script(
    script_path: &Path,
    fixture: &Path,
) -> Result<ValidationOutput, CliError> {
    let workdir = isolated_workdir()?;
    fs::create_dir_all(&workdir)?;
    let result = run_python_script(script_path, fixture, &workdir, VALIDATION_TIMEOUT);
    let _ = fs::remove_dir_all(&workdir);
    result
}

fn run_python_script(
    script_path: &Path,
    fixture: &Path,
    workdir: &Path,
    timeout: Duration,
) -> Result<ValidationOutput, CliError> {
    let mut child = Command::new("/usr/bin/env")
        .arg("python3")
        .arg(script_path)
        .env("SYNTH_INPUT", fixture)
        .current_dir(workdir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            CliError::Core(format!(
                "failed to run candidate script {}: {error}",
                script_path.display()
            ))
        })?;
    let started = SystemTime::now();

    loop {
        if child.try_wait()?.is_some() {
            let output = child.wait_with_output()?;
            return Ok(ValidationOutput {
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                exit_code: output.status.code(),
                timed_out: false,
            });
        }

        if started.elapsed().unwrap_or_default() >= timeout {
            let _ = child.kill();
            let output = child.wait_with_output()?;
            return Ok(ValidationOutput {
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                exit_code: output.status.code(),
                timed_out: true,
            });
        }

        thread::sleep(Duration::from_millis(20));
    }
}

fn isolated_workdir() -> Result<PathBuf, CliError> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok(std::env::temp_dir().join(format!("agentflow-synth-{}-{nanos}", std::process::id())))
}

fn synthesized_tool_yaml(name: &str, description: &str, script_path: &Path) -> String {
    let description = yaml_single_line(description);
    let maturity = ToolMaturity::Exploratory.as_str();
    format!(
        r#"schema_version: {}
namespace: synth
name: {}
version: {}
maturity: {}
description: {}
params:
  input:
    type: string
    required: true
outputs:
  result:
    type: Text
runtime:
  backend: local
  command:
    - /usr/bin/env
    - python3
    - {}
"#,
        agentflow_schemas::TOOL_SCHEMA_V0,
        name,
        SYNTH_VERSION,
        maturity,
        description,
        script_path.display()
    )
}

fn yaml_single_line(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\n' | '\r' | '\t' | '#' => ' ',
            ch => ch,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_exit_status(status: &ExitStatus) -> String {
    status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn stderr_summary(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        "no stderr".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: Vec<String>) -> Vec<OsString> {
        items.into_iter().map(OsString::from).collect()
    }

    fn temp_project_path(test_name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "agentflow-cli-synth-{test_name}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn init_project(path: &Path) {
        crate::run(args(vec![
            "agentflow".to_string(),
            "init".to_string(),
            "--name".to_string(),
            "Synth Demo".to_string(),
            "--path".to_string(),
            path.display().to_string(),
        ]))
        .unwrap();
    }

    fn write_fixture(path: &Path, contents: &str) -> PathBuf {
        let fixture = path.join("fixture.txt");
        fs::write(&fixture, contents).unwrap();
        fixture
    }

    fn write_stub_synthesizer(path: &Path, name: &str, candidate: &str) -> PathBuf {
        let stub = path.join(name);
        fs::write(
            &stub,
            format!(
                r#"#!/bin/sh
cat <<'PY'
{candidate}
PY
"#
            ),
        )
        .unwrap();
        stub
    }

    fn synth_args(
        path: &Path,
        fixture: &Path,
        synthesizer: &str,
        name: &str,
        expect: &str,
    ) -> Vec<OsString> {
        args(vec![
            "agentflow".to_string(),
            "synth".to_string(),
            "--name".to_string(),
            name.to_string(),
            "--description".to_string(),
            "Echo the input file exactly".to_string(),
            "--fixture".to_string(),
            fixture.display().to_string(),
            "--expect".to_string(),
            expect.to_string(),
            "--synthesizer".to_string(),
            synthesizer.to_string(),
            "--path".to_string(),
            path.display().to_string(),
        ])
    }

    #[test]
    fn synth_validates_and_registers_exploratory_tool() {
        let path = temp_project_path("validated");
        init_project(&path);
        let fixture = write_fixture(&path, "expected-line\n");
        let candidate = r#"import os
from pathlib import Path
print(Path(os.environ["SYNTH_INPUT"]).read_text(), end="")"#;
        let stub = write_stub_synthesizer(&path, "stub_good.sh", candidate);
        let synthesizer = format!("/bin/sh {}", stub.display());

        let output = crate::run(synth_args(
            &path,
            &fixture,
            &synthesizer,
            "echo_input",
            "expected-line",
        ))
        .unwrap();

        assert!(output.contains("VALIDATED"));
        assert!(output.contains("synth/echo_input"));
        assert!(output.contains("exploratory"));

        let list = crate::run(args(vec![
            "agentflow".to_string(),
            "tools".to_string(),
            "list".to_string(),
            "--path".to_string(),
            path.display().to_string(),
        ]))
        .unwrap();
        assert!(list.contains("synth/echo_input@0.1.0 [exploratory]"));

        let inspect = crate::run(args(vec![
            "agentflow".to_string(),
            "tools".to_string(),
            "inspect".to_string(),
            "synth/echo_input".to_string(),
            "--json".to_string(),
            "--path".to_string(),
            path.display().to_string(),
        ]))
        .unwrap();
        assert!(inspect.contains("\"maturity\":\"exploratory\""));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn synth_rejects_unvalidated_script_without_registering() {
        let path = temp_project_path("rejected");
        init_project(&path);
        let fixture = write_fixture(&path, "expected-line\n");
        let stub = write_stub_synthesizer(&path, "stub_bad.sh", r#"print("wrong output")"#);
        let synthesizer = format!("/bin/sh {}", stub.display());

        let output = crate::run(synth_args(
            &path,
            &fixture,
            &synthesizer,
            "bad_echo",
            "expected-line",
        ))
        .unwrap();

        assert!(output.contains("REJECTED"));
        assert!(output.contains("wrong output"));
        assert!(path.join(".agentflow/synth/bad_echo.py").exists());

        let list = crate::run(args(vec![
            "agentflow".to_string(),
            "tools".to_string(),
            "list".to_string(),
            "--path".to_string(),
            path.display().to_string(),
        ]))
        .unwrap();
        assert_eq!(list, "No tools registered");

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn synth_missing_synthesizer_errors_without_registering() {
        let path = temp_project_path("missing-synthesizer");
        init_project(&path);
        let fixture = write_fixture(&path, "expected-line\n");

        let error = crate::run(synth_args(
            &path,
            &fixture,
            "/definitely/missing-agentflow-synthesizer",
            "missing_backend",
            "expected-line",
        ))
        .unwrap_err();

        assert!(error.message().contains("failed to run synthesizer"));
        assert!(!path.join(".agentflow/synth/missing_backend.py").exists());

        let list = crate::run(args(vec![
            "agentflow".to_string(),
            "tools".to_string(),
            "list".to_string(),
            "--path".to_string(),
            path.display().to_string(),
        ]))
        .unwrap();
        assert_eq!(list, "No tools registered");

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn strip_markdown_fence_removes_python_fence() {
        let candidate = r#"```python
print("ok")
```"#;

        assert_eq!(strip_markdown_fence(candidate), "print(\"ok\")");
    }
}
