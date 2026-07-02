use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct TempProject {
    path: PathBuf,
}

impl TempProject {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("agentflow-jobs-cli-{}-{name}", std::process::id()));
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

fn write_detached_tool(project: &Path) -> PathBuf {
    let submit_script = project.join("submit.sh");
    fs::write(
        &submit_script,
        "#!/bin/sh\nprintf 'job_handle=job-1\\n'\nexit 0\n",
    )
    .expect("submit script should be written");
    let poll_script = project.join("poll.sh");
    fs::write(&poll_script, "#!/bin/sh\nprintf 'status=running\\n'\n")
        .expect("poll script should be written");

    let tool_path = project.join("detached_note.tool.yaml");
    fs::write(
        &tool_path,
        format!(
            r#"
schema_version: agentflow.tool.v0
namespace: async
name: detached_note
version: 0.1.0
maturity: wrapped
description: Detached note
outputs:
  note:
    type: Text
runtime:
  backend: detached
  command:
    - /bin/sh
    - {}
  poll:
    - /bin/sh
    - {}
"#,
            submit_script.display(),
            poll_script.display()
        ),
    )
    .expect("tool spec should be written");
    tool_path
}

fn write_cancellable_detached_tool(project: &Path, cancel_marker: &Path) -> PathBuf {
    let submit_script = project.join("submit.sh");
    fs::write(
        &submit_script,
        "#!/bin/sh\nprintf 'job_handle=job-1\\n'\nexit 0\n",
    )
    .expect("submit script should be written");
    let poll_script = project.join("poll.sh");
    fs::write(&poll_script, "#!/bin/sh\nprintf 'status=running\\n'\n")
        .expect("poll script should be written");
    let cancel_script = project.join("cancel.sh");
    fs::write(
        &cancel_script,
        format!("#!/bin/sh\ntouch {}\nexit 0\n", cancel_marker.display()),
    )
    .expect("cancel script should be written");

    let tool_path = project.join("cancellable_note.tool.yaml");
    fs::write(
        &tool_path,
        format!(
            r#"
schema_version: agentflow.tool.v0
namespace: async
name: cancellable_note
version: 0.1.0
maturity: wrapped
description: Cancellable detached note
outputs:
  note:
    type: Text
runtime:
  backend: detached
  command:
    - /bin/sh
    - {}
  poll:
    - /bin/sh
    - {}
  cancel:
    - /bin/sh
    - {}
"#,
            submit_script.display(),
            poll_script.display(),
            cancel_script.display()
        ),
    )
    .expect("tool spec should be written");
    tool_path
}

fn json_string_field(json: &str, key: &str) -> String {
    let needle = format!("\"{key}\":\"");
    let start = json
        .find(&needle)
        .unwrap_or_else(|| panic!("expected {key} in {json}"))
        + needle.len();
    let end = json[start..]
        .find('"')
        .unwrap_or_else(|| panic!("unterminated {key} value in {json}"));
    json[start..start + end].to_string()
}

fn write_detached_flow(project: &Path, flow_id: &str) -> PathBuf {
    let flow_path = project.join(format!("{flow_id}.flow.yaml"));
    fs::write(
        &flow_path,
        format!(
            r#"
schema_version: agentflow.flow.v0
id: {flow_id}
name: Jobs CLI demo
steps:
  - id: submit
    tool: async/detached_note
    needs: []
    outputs:
      note: submitted_note
"#
        ),
    )
    .expect("flow should be written");
    flow_path
}

#[test]
fn jobs_list_and_poll_report_an_outstanding_detached_job() {
    let project = TempProject::new("list-poll");
    let project_arg = project.path.to_str().expect("temp path should be UTF-8");
    let flow_id = "jobs_cli_demo";

    run_agentflow(["init", "--name", "Jobs CLI Demo", "--path", project_arg]);

    let tool_path = write_detached_tool(&project.path);
    run_agentflow([
        "tools",
        "register",
        tool_path.to_str().expect("tool path should be UTF-8"),
        "--path",
        project_arg,
    ]);

    let flow_path = write_detached_flow(&project.path, flow_id);
    run_agentflow([
        "flow",
        "approve",
        flow_path.to_str().expect("flow path should be UTF-8"),
        "--path",
        project_arg,
    ]);

    // Before anything is submitted, no outstanding jobs.
    let empty_list = run_agentflow(["jobs", "list", "--path", project_arg]);
    assert!(empty_list.contains("_none_"), "{empty_list}");

    let run = run_agentflow(["run", flow_id, "--path", project_arg]);
    assert!(
        run.contains("still running (detached)"),
        "run output should note the outstanding job: {run}"
    );

    // `jobs list` (project-wide, no --flow filter) reports the outstanding job.
    let list = run_agentflow(["jobs", "list", "--path", project_arg]);
    assert!(list.contains("job-1"), "{list}");
    assert!(list.contains("submit"), "{list}");

    let list_json = run_agentflow(["jobs", "list", "--json", "--path", project_arg]);
    assert!(
        list_json.contains("\"agentflow.jobs_list.v0\""),
        "{list_json}"
    );
    assert!(
        list_json.contains("\"job_handle\":\"job-1\""),
        "{list_json}"
    );

    // `jobs list --flow` scopes to a single flow (still finds it here).
    let list_scoped = run_agentflow(["jobs", "list", "--flow", flow_id, "--path", project_arg]);
    assert!(list_scoped.contains("job-1"), "{list_scoped}");

    // `jobs poll` polls without advancing the flow; the fixture poll script
    // always reports `running`, so the job stays outstanding afterward.
    let poll = run_agentflow(["jobs", "poll", "--path", project_arg]);
    assert!(poll.contains("Polled: 1"), "{poll}");
    assert!(poll.contains("Still running: 1"), "{poll}");

    let list_after_poll = run_agentflow(["jobs", "list", "--path", project_arg]);
    assert!(
        list_after_poll.contains("job-1"),
        "a still-running job should remain listed: {list_after_poll}"
    );
}

#[test]
fn jobs_cancel_finalizes_a_submitted_attempt_and_runs_the_cancel_command() {
    let project = TempProject::new("cancel");
    let project_arg = project.path.to_str().expect("temp path should be UTF-8");
    let flow_id = "jobs_cli_cancel_demo";
    let cancel_marker = project.path.join("cancel-was-called");

    run_agentflow([
        "init",
        "--name",
        "Jobs CLI Cancel Demo",
        "--path",
        project_arg,
    ]);

    let tool_path = write_cancellable_detached_tool(&project.path, &cancel_marker);
    run_agentflow([
        "tools",
        "register",
        tool_path.to_str().expect("tool path should be UTF-8"),
        "--path",
        project_arg,
    ]);

    let flow_path = project.path.join(format!("{flow_id}.flow.yaml"));
    fs::write(
        &flow_path,
        format!(
            r#"
schema_version: agentflow.flow.v0
id: {flow_id}
name: Jobs CLI cancel demo
steps:
  - id: submit
    tool: async/cancellable_note
    needs: []
    outputs:
      note: submitted_note
"#
        ),
    )
    .expect("flow should be written");
    run_agentflow([
        "flow",
        "approve",
        flow_path.to_str().expect("flow path should be UTF-8"),
        "--path",
        project_arg,
    ]);

    run_agentflow(["run", flow_id, "--path", project_arg]);

    let list_json = run_agentflow(["jobs", "list", "--json", "--path", project_arg]);
    let attempt_id = json_string_field(&list_json, "attempt_id");

    let cancel = run_agentflow(["jobs", "cancel", &attempt_id, "--path", project_arg]);
    assert!(cancel.contains("Cancelled"), "{cancel}");
    assert!(cancel.contains(&attempt_id), "{cancel}");
    assert!(
        cancel_marker.exists(),
        "the tool's cancel command should have run"
    );

    let list_after_cancel = run_agentflow(["jobs", "list", "--path", project_arg]);
    assert!(
        list_after_cancel.contains("_none_"),
        "a cancelled job should no longer be outstanding: {list_after_cancel}"
    );

    // Cancelling the same (now-terminal) attempt id again is rejected — it's
    // no longer an outstanding submitted attempt.
    let (success, _stdout, stderr) =
        try_agentflow(["jobs", "cancel", &attempt_id, "--path", project_arg]);
    assert!(!success, "cancelling twice should fail");
    assert!(stderr.contains("outstanding submitted attempt"), "{stderr}");
}
