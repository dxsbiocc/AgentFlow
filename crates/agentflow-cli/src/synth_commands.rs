use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use agentflow_core::domain::ToolMaturity;
use agentflow_core::storage::{ProjectStore, ToolSpec};

use crate::cli_args::SynthArgs;
use crate::{last_value, CliError};

pub(crate) const DEFAULT_SYNTHESIZER: &str = "claude -p";
const SYNTH_VERSION: &str = "0.1.0";
const VALIDATION_TIMEOUT: Duration = Duration::from_secs(60);
const VALIDATION_PATH: &str = "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin";

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
    result_output: Option<String>,
}

#[derive(Debug)]
struct AutoSynthCandidate {
    script: String,
    fixture: String,
    expect: String,
}

pub(crate) enum AutoSynthToolResult {
    Registered(String),
    Rejected(String),
}

pub(crate) fn synth_command(args: SynthArgs) -> Result<String, CliError> {
    let options = SynthOptions {
        name: last_value(args.name),
        description: last_value(args.description),
        fixture: last_value(args.fixture),
        expect: last_value(args.expect),
        synthesizer: last_value(args.synthesizer),
        path: last_value(args.project.path),
    };
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
    let synthesizer = configured_or_default_synthesizer(store.root_path(), options.synthesizer)?;
    let candidate = run_project_synthesizer(store.root_path(), &synthesizer, &prompt)?;
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

pub(crate) fn auto_synthesize_agent_tool(
    store: &ProjectStore,
    synthesizer: &str,
    hypothesis_statement: &str,
    capability_need: &str,
) -> Result<AutoSynthToolResult, CliError> {
    let prompt = build_auto_synth_prompt(hypothesis_statement, capability_need);
    let raw_candidate = run_project_synthesizer(store.root_path(), synthesizer, &prompt)?;
    let candidate = match parse_auto_synth_candidate(&raw_candidate) {
        Ok(candidate) => candidate,
        Err(error) => return Ok(AutoSynthToolResult::Rejected(error.message())),
    };

    let name = auto_synth_tool_name(hypothesis_statement, capability_need)?;
    let description = format!(
        "Auto-synthesized tool for hypothesis {hypothesis_statement}. Capability need {capability_need}"
    );
    let script_path = synth_script_path(store.root_path(), &name);
    let fixture_path = auto_synth_fixture_path(store.root_path(), &name);
    if let Some(parent) = script_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&script_path, candidate.script.as_bytes())?;
    fs::write(&fixture_path, candidate.fixture.as_bytes())?;
    let script_path = fs::canonicalize(&script_path)?;
    let fixture_path = fs::canonicalize(&fixture_path)?;

    let fixture_validation = validate_candidate_script(&script_path, &fixture_path)?;
    if !auto_synth_validation_passed(&fixture_validation, &candidate.expect) {
        cleanup_auto_synth_candidate(&script_path, &fixture_path);
        return Ok(AutoSynthToolResult::Rejected(auto_synth_rejection_reason(
            "fixture smoke",
            &fixture_validation,
            &candidate.expect,
        )));
    }
    let runtime_validation = validate_runtime_candidate_script(&script_path)?;
    if !auto_synth_validation_passed(&runtime_validation, &candidate.expect) {
        cleanup_auto_synth_candidate(&script_path, &fixture_path);
        return Ok(AutoSynthToolResult::Rejected(auto_synth_rejection_reason(
            "runtime smoke",
            &runtime_validation,
            &candidate.expect,
        )));
    }

    let spec_yaml = synthesized_agent_tool_yaml(&name, &description, &script_path);
    let spec = ToolSpec::from_simple_yaml(&spec_yaml)?;
    let registration = store.register_tool(spec)?;
    Ok(AutoSynthToolResult::Registered(registration.tool_ref))
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

fn build_auto_synth_prompt(hypothesis_statement: &str, capability_need: &str) -> String {
    format!(
        concat!(
            "You are writing an AgentFlow exploratory analysis tool. Use only Python 3 standard library.\n",
            "The tool must be self-contained and deterministic for a smoke fixture.\n",
            "At runtime, write the main Markdown/Text result to the file path in AGENTFLOW_OUTPUT_RESULT.\n",
            "Also print the same result to stdout so smoke validation can inspect it.\n",
            "The registered tool will NOT receive SYNTH_INPUT at runtime, so it must still succeed when SYNTH_INPUT is unset.\n",
            "You may optionally read a smoke fixture from SYNTH_INPUT during validation, but do not require network access.\n\n",
            "Research hypothesis:\n{}\n\n",
            "Capability gap:\n{}\n\n",
            "Return exactly three sections with these markers and no extra text:\n",
            "===SCRIPT===\n",
            "<raw Python code>\n",
            "===FIXTURE===\n",
            "<small fixture text for SYNTH_INPUT>\n",
            "===EXPECT===\n",
            "<one substring that must appear in stdout and AGENTFLOW_OUTPUT_RESULT>\n"
        ),
        hypothesis_statement,
        capability_need
    )
}

fn parse_auto_synth_candidate(candidate: &str) -> Result<AutoSynthCandidate, CliError> {
    let script = strip_markdown_fence(&required_section(candidate, "SCRIPT")?);
    let fixture = strip_markdown_fence(&required_section(candidate, "FIXTURE")?);
    let expect = required_section(candidate, "EXPECT")?
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string();

    if script.trim().is_empty() || fixture.trim().is_empty() || expect.trim().is_empty() {
        return Err(CliError::Core(
            "auto-synth candidate is missing script, fixture, or expect section".to_string(),
        ));
    }

    Ok(AutoSynthCandidate {
        script,
        fixture,
        expect,
    })
}

fn required_section(candidate: &str, name: &str) -> Result<String, CliError> {
    let marker = format!("==={name}===");
    let mut in_section = false;
    let mut lines = Vec::new();
    for line in candidate.lines() {
        let trimmed = line.trim();
        if trimmed == marker {
            in_section = true;
            continue;
        }
        if trimmed.starts_with("===") && trimmed.ends_with("===") && in_section {
            break;
        }
        if in_section {
            lines.push(line);
        }
    }
    if !in_section {
        return Err(CliError::Core(format!(
            "auto-synth candidate is missing {marker}"
        )));
    }
    Ok(lines.join("\n").trim().to_string())
}

pub(crate) fn run_project_synthesizer(
    project_root: &Path,
    command_line: &str,
    prompt: &str,
) -> Result<String, CliError> {
    let env = crate::llm_commands::load_project_llm_env(project_root)?;
    run_synthesizer_with_env(command_line, prompt, &env)
}

pub(crate) fn configured_or_default_synthesizer(
    project_root: &Path,
    explicit: Option<String>,
) -> Result<String, CliError> {
    if let Some(explicit) = explicit {
        return Ok(explicit);
    }
    Ok(crate::llm_commands::configured_synthesizer(project_root)?
        .unwrap_or_else(|| DEFAULT_SYNTHESIZER.to_string()))
}

fn run_synthesizer_with_env(
    command_line: &str,
    prompt: &str,
    env: &[crate::llm_commands::LlmEnvEntry],
) -> Result<String, CliError> {
    let argv = split_synthesizer_command(command_line)?;
    let mut command = Command::new(&argv[0]);
    for entry in env {
        command.env(&entry.key, &entry.value);
    }
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
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Quote {
        Single,
        Double,
    }

    let mut argv = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut arg_started = false;
    let mut chars = command_line.chars().peekable();

    while let Some(ch) = chars.next() {
        match quote {
            Some(Quote::Single) => {
                if ch == '\'' {
                    quote = None;
                } else {
                    current.push(ch);
                }
            }
            Some(Quote::Double) => match ch {
                '"' => quote = None,
                '\\' => {
                    let Some(next) = chars.peek().copied() else {
                        return Err(CliError::InvalidArgument(
                            "unterminated escape in --synthesizer".to_string(),
                        ));
                    };
                    if matches!(next, '"' | '\\' | '$' | '`' | '\n') {
                        current.push(chars.next().expect("peeked synthesizer char"));
                    } else {
                        current.push(ch);
                    }
                }
                _ => current.push(ch),
            },
            None => match ch {
                ch if ch.is_whitespace() => {
                    if arg_started {
                        argv.push(std::mem::take(&mut current));
                        arg_started = false;
                    }
                }
                '\'' => {
                    quote = Some(Quote::Single);
                    arg_started = true;
                }
                '"' => {
                    quote = Some(Quote::Double);
                    arg_started = true;
                }
                '\\' => {
                    let Some(next) = chars.next() else {
                        return Err(CliError::InvalidArgument(
                            "unterminated escape in --synthesizer".to_string(),
                        ));
                    };
                    current.push(next);
                    arg_started = true;
                }
                _ => {
                    current.push(ch);
                    arg_started = true;
                }
            },
        }
    }

    if quote.is_some() {
        return Err(CliError::InvalidArgument(
            "unterminated quote in --synthesizer".to_string(),
        ));
    }
    if arg_started {
        argv.push(current);
    }
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

fn auto_synth_fixture_path(project_root: &Path, name: &str) -> PathBuf {
    project_root
        .join(".agentflow")
        .join("synth")
        .join(format!("{name}.fixture.txt"))
}

fn cleanup_auto_synth_candidate(script_path: &Path, fixture_path: &Path) {
    let _ = fs::remove_file(script_path);
    let _ = fs::remove_file(fixture_path);
}

fn validate_candidate_script(
    script_path: &Path,
    fixture: &Path,
) -> Result<ValidationOutput, CliError> {
    let workdir = isolated_workdir()?;
    fs::create_dir_all(&workdir)?;
    let result = run_python_script(script_path, Some(fixture), &workdir, VALIDATION_TIMEOUT);
    let _ = fs::remove_dir_all(&workdir);
    result
}

fn validate_runtime_candidate_script(script_path: &Path) -> Result<ValidationOutput, CliError> {
    let workdir = isolated_workdir()?;
    fs::create_dir_all(&workdir)?;
    let result = run_python_script(script_path, None, &workdir, VALIDATION_TIMEOUT);
    let _ = fs::remove_dir_all(&workdir);
    result
}

fn run_python_script(
    script_path: &Path,
    fixture: Option<&Path>,
    workdir: &Path,
    timeout: Duration,
) -> Result<ValidationOutput, CliError> {
    let result_path = workdir.join("result.txt");
    let mut command = Command::new("/usr/bin/env");
    command
        .env_clear()
        .env("PATH", VALIDATION_PATH)
        .env("AGENTFLOW_WORKDIR", workdir)
        .env("AGENTFLOW_OUTPUT_RESULT", &result_path)
        .arg("python3")
        .arg(script_path)
        .current_dir(workdir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(fixture) = fixture {
        command.env("SYNTH_INPUT", fixture);
    }
    configure_child_process_group(&mut command);
    let mut child = command.spawn().map_err(|error| {
        CliError::Core(format!(
            "failed to run candidate script {}: {error}",
            script_path.display()
        ))
    })?;
    let started = SystemTime::now();

    loop {
        if child.try_wait()?.is_some() {
            let output = child.wait_with_output()?;
            let result_output = fs::read_to_string(&result_path).ok();
            return Ok(ValidationOutput {
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                exit_code: output.status.code(),
                timed_out: false,
                result_output,
            });
        }

        if started.elapsed().unwrap_or_default() >= timeout {
            kill_child_process_group(&mut child);
            let output = child.wait_with_output()?;
            let result_output = fs::read_to_string(&result_path).ok();
            return Ok(ValidationOutput {
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                exit_code: output.status.code(),
                timed_out: true,
                result_output,
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

fn synthesized_agent_tool_yaml(name: &str, description: &str, script_path: &Path) -> String {
    let description = yaml_single_line(description);
    let maturity = ToolMaturity::Exploratory.as_str();
    format!(
        r#"schema_version: {}
namespace: synth
name: {}
version: {}
maturity: {}
description: {}
outputs:
  result:
    type: Markdown
    observer: artifact_summary
runtime:
  backend: local
  timeout_seconds: 60
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

fn auto_synth_validation_passed(validation: &ValidationOutput, expect: &str) -> bool {
    validation.exit_code == Some(0)
        && !validation.timed_out
        && validation.stdout.trim().contains(expect)
        && validation
            .result_output
            .as_deref()
            .is_some_and(|output| !output.trim().is_empty() && output.contains(expect))
}

fn auto_synth_rejection_reason(phase: &str, validation: &ValidationOutput, expect: &str) -> String {
    format!(
        concat!(
            "candidate failed {}: ",
            "exit_code={}, timed_out={}, expected={}, stdout={}, stderr={}"
        ),
        phase,
        validation
            .exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "unknown".to_string()),
        validation.timed_out,
        expect,
        snippet(&validation.stdout),
        snippet(&validation.stderr)
    )
}

fn snippet(value: &str) -> String {
    let trimmed = value.trim();
    let boundary = trimmed
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= 240)
        .last()
        .unwrap_or(0);
    if trimmed.len() <= 240 {
        trimmed.to_string()
    } else {
        format!("{}…", &trimmed[..boundary])
    }
}

fn configure_child_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        command.process_group(0);
    }
}

fn kill_child_process_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("/bin/kill")
            .arg("-TERM")
            .arg(format!("-{}", child.id()))
            .status();
    }
    let _ = child.kill();
}

fn auto_synth_tool_name(
    hypothesis_statement: &str,
    capability_need: &str,
) -> Result<String, CliError> {
    let mut slug = String::new();
    let mut previous_separator = false;
    for ch in hypothesis_statement
        .chars()
        .chain(std::iter::once(' '))
        .chain(capability_need.chars())
    {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            previous_separator = false;
        } else if !previous_separator && !slug.is_empty() {
            slug.push('_');
            previous_separator = true;
        }
        if slug.len() >= 40 {
            break;
        }
    }
    let slug = slug.trim_matches('_');
    let slug = if slug.is_empty() { "tool" } else { slug };
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = format!("auto_synth_{slug}_{:x}", nanos % 0xffff_ffff);
    validate_tool_name(&name)?;
    Ok(name)
}

fn yaml_single_line(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\n' | '\r' | '\t' | '#' | ':' => ' ',
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
    use std::ffi::OsString;

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
    fn split_synthesizer_command_respects_shell_quotes() {
        assert_eq!(
            split_synthesizer_command("/bin/sh '/tmp/project with space/synth.sh' --flag").unwrap(),
            vec![
                "/bin/sh".to_string(),
                "/tmp/project with space/synth.sh".to_string(),
                "--flag".to_string()
            ]
        );
        assert_eq!(
            split_synthesizer_command(r#"python3 "two words.py""#).unwrap(),
            vec!["python3".to_string(), "two words.py".to_string()]
        );
    }

    #[test]
    fn split_synthesizer_command_rejects_unterminated_quotes() {
        let error = split_synthesizer_command("python3 'missing-end").unwrap_err();
        assert!(error.message().contains("unterminated quote"));
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
    fn auto_synth_rejects_failed_smoke_without_leaving_script() {
        let path = temp_project_path("auto-rejected");
        init_project(&path);
        let store = ProjectStore::open(&path).unwrap();
        let candidate = r#"===SCRIPT===
print("wrong output")
===FIXTURE===
fixture,line
===EXPECT===
expected-line
"#;
        let stub = write_stub_synthesizer(&path, "stub_auto_bad.sh", candidate);
        let synthesizer = format!("/bin/sh {}", stub.display());

        let outcome = auto_synthesize_agent_tool(
            &store,
            &synthesizer,
            "Auto synth cleanup hypothesis",
            "Need a custom rejected tool",
        )
        .unwrap();

        match outcome {
            AutoSynthToolResult::Rejected(reason) => assert!(reason.contains("fixture smoke")),
            AutoSynthToolResult::Registered(tool_ref) => {
                panic!("unexpected auto-synth registration: {tool_ref}")
            }
        }
        let synth_entries = fs::read_dir(path.join(".agentflow/synth"))
            .map(|entries| entries.count())
            .unwrap_or_default();
        assert_eq!(synth_entries, 0);

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn auto_synth_rejects_fixture_dependent_runtime_script() {
        let path = temp_project_path("auto-runtime-rejected");
        init_project(&path);
        let store = ProjectStore::open(&path).unwrap();
        let candidate = r#"===SCRIPT===
import os
from pathlib import Path

result = Path(os.environ["SYNTH_INPUT"]).read_text()
output_path = os.environ.get("AGENTFLOW_OUTPUT_RESULT")
if output_path:
    Path(output_path).write_text(result, encoding="utf-8")
print(result, end="")
===FIXTURE===
fixture-runtime-ok
===EXPECT===
fixture-runtime-ok
"#;
        let stub = write_stub_synthesizer(&path, "stub_auto_fixture_only.sh", candidate);
        let synthesizer = format!("/bin/sh {}", stub.display());

        let outcome = auto_synthesize_agent_tool(
            &store,
            &synthesizer,
            "Auto synth runtime parity hypothesis",
            "Need a custom rejected runtime tool",
        )
        .unwrap();

        match outcome {
            AutoSynthToolResult::Rejected(reason) => assert!(reason.contains("runtime smoke")),
            AutoSynthToolResult::Registered(tool_ref) => {
                panic!("unexpected auto-synth registration: {tool_ref}")
            }
        }
        let synth_entries = fs::read_dir(path.join(".agentflow/synth"))
            .map(|entries| entries.count())
            .unwrap_or_default();
        assert_eq!(synth_entries, 0);

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn validation_env_does_not_inherit_host_home() {
        let path = temp_project_path("validation-env-clear");
        init_project(&path);
        let script = path.join("env_clear.py");
        fs::write(
            &script,
            r#"import os
from pathlib import Path

if os.environ.get("HOME"):
    raise SystemExit("HOME leaked into validation")
result = "ENV_CLEARED_OK\n"
output_path = os.environ.get("AGENTFLOW_OUTPUT_RESULT")
if output_path:
    Path(output_path).write_text(result, encoding="utf-8")
print(result, end="")
"#,
        )
        .unwrap();
        let fixture = write_fixture(&path, "unused\n");

        let validation = validate_candidate_script(&script, &fixture).unwrap();

        assert_eq!(validation.exit_code, Some(0), "{validation:?}");
        assert!(validation.stdout.contains("ENV_CLEARED_OK"));
        assert!(validation
            .result_output
            .as_deref()
            .is_some_and(|result| result.contains("ENV_CLEARED_OK")));

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
