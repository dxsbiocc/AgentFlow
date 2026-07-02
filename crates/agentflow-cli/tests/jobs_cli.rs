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
