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
            "agentflow-composition-staging-{}-{name}",
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

#[derive(Debug)]
struct AttemptLine {
    status: String,
    workdir: PathBuf,
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

fn json_string_field(fragment: &str, field: &str) -> String {
    let marker = format!("\"{field}\":\"");
    fragment
        .split(&marker)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("expected JSON string field")
        .to_string()
}

fn attempt_for_step(output: &str, step_id: &str) -> AttemptLine {
    let line = output
        .lines()
        .find(|line| line.contains(step_id))
        .expect("expected run attempt for step");
    let parts = line.split_whitespace().collect::<Vec<_>>();
    assert!(
        parts.len() >= 5,
        "expected attempt line with status and workdir: {line}"
    );
    AttemptLine {
        status: parts[3].trim_matches(['[', ']']).to_string(),
        workdir: PathBuf::from(parts[4]),
    }
}

fn computed_artifact_path_for_source(
    artifacts_json: &str,
    source_step_id: &str,
) -> Option<PathBuf> {
    let source_marker = format!("\"source_step_id\":\"{source_step_id}\"");
    artifacts_json
        .split("{\"id\":\"")
        .skip(1)
        .find(|artifact| {
            artifact.contains("\"kind\":\"computed\"") && artifact.contains(&source_marker)
        })
        .map(|artifact| PathBuf::from(json_string_field(artifact, "path")))
}

fn write_composition_tools(project: &Path) -> (PathBuf, PathBuf) {
    let producer_script = project.join("producer.sh");
    fs::write(
        &producer_script,
        r#"set -eu
cat "$AGENTFLOW_INPUT_SEED" > "$AGENTFLOW_OUTPUT_REPORT"
printf '\nproducer=ok\n' >> "$AGENTFLOW_OUTPUT_REPORT"
printf 'producer ok\n'
"#,
    )
    .expect("producer script should be written");

    let consumer_script = project.join("consumer.sh");
    fs::write(
        &consumer_script,
        r#"set -eu
case "$AGENTFLOW_INPUT_UPSTREAM_REPORT" in
  "$AGENTFLOW_WORKDIR"/inputs/upstream_report/*) ;;
  *) echo "consumer input was not staged inside workdir" >&2; exit 9 ;;
esac
cat "$AGENTFLOW_INPUT_UPSTREAM_REPORT" > "$AGENTFLOW_OUTPUT_FINAL_REPORT"
printf '\nconsumer=ok\n' >> "$AGENTFLOW_OUTPUT_FINAL_REPORT"
printf 'consumer ok\n'
"#,
    )
    .expect("consumer script should be written");

    let producer_spec = project.join("producer.tool.yaml");
    fs::write(
        &producer_spec,
        format!(
            r#"
schema_version: agentflow.tool.v0
namespace: synthetic
name: producer
version: 0.1.0
maturity: wrapped
description: Copy a declared seed input to a declared report output.
inputs:
  seed:
    type: Markdown
    required: true
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/sh
    - {}
"#,
            producer_script.display()
        ),
    )
    .expect("producer tool spec should be written");

    let consumer_spec = project.join("consumer.tool.yaml");
    fs::write(
        &consumer_spec,
        format!(
            r#"
schema_version: agentflow.tool.v0
namespace: synthetic
name: consumer
version: 0.1.0
maturity: wrapped
description: Consume the producer report and write a final report.
inputs:
  upstream_report:
    type: Markdown
    required: true
outputs:
  final_report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/sh
    - {}
"#,
            consumer_script.display()
        ),
    )
    .expect("consumer tool spec should be written");

    (producer_spec, consumer_spec)
}

fn write_seed_artifact(project: &Path) -> PathBuf {
    let seed_path = project.join("seed.md");
    fs::write(&seed_path, "# seed\npayload=composition-staging\n")
        .expect("seed artifact should be written");
    seed_path
}

fn write_composition_flow(project: &Path, flow_id: &str, seed_id: &str) -> PathBuf {
    let flow_path = project.join(format!("{flow_id}.flow.yaml"));
    fs::write(
        &flow_path,
        format!(
            r#"
schema_version: agentflow.flow.v0
id: {flow_id}
name: Composition staging demo
steps:
  - id: producer
    tool: synthetic/producer
    reason: Produce a deterministic report from an imported local seed.
    needs: []
    inputs:
      seed: {seed_id}
    outputs:
      report: report
  - id: consumer
    tool: synthetic/consumer
    reason: Consume the producer report through flow composition syntax.
    needs: [producer]
    inputs:
      upstream_report: producer.report
    outputs:
      final_report: final_report
"#
        ),
    )
    .expect("flow should be written");
    flow_path
}

fn project_artifact_path(project: &Path, display_path: PathBuf) -> PathBuf {
    if display_path.is_absolute() {
        display_path
    } else {
        project.join(display_path)
    }
}

#[test]
fn flow_composition_stages_step_output_inputs_inside_consumer_workdir() {
    let project = TempProject::new("two-step");
    let project_arg = project.path.to_str().expect("temp path should be UTF-8");
    let flow_id = "compose_stage_demo";

    run_agentflow([
        "init",
        "--name",
        "Composition Staging",
        "--path",
        project_arg,
    ]);

    let (producer_spec, consumer_spec) = write_composition_tools(&project.path);
    run_agentflow([
        "tools",
        "register",
        producer_spec.to_str().expect("tool path should be UTF-8"),
        "--path",
        project_arg,
    ]);
    run_agentflow([
        "tools",
        "register",
        consumer_spec.to_str().expect("tool path should be UTF-8"),
        "--path",
        project_arg,
    ]);

    let seed_path = write_seed_artifact(&project.path);
    let seed_import = run_agentflow([
        "import",
        seed_path.to_str().expect("seed path should be UTF-8"),
        "--type",
        "Markdown",
        "--path",
        project_arg,
    ]);
    let seed_id = extract_after_line(&seed_import, "Id: ");

    let flow_path = write_composition_flow(&project.path, flow_id, &seed_id);
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

    let run = run_agentflow(["run", flow_id, "--path", project_arg]);
    assert!(run.contains("Completed steps: 2"), "{run}");
    assert!(run.contains("Failed steps: 0"), "{run}");

    let producer_step_id = format!("step:{flow_id}/producer");
    let consumer_step_id = format!("step:{flow_id}/consumer");
    let producer_attempt = attempt_for_step(&run, &producer_step_id);
    let consumer_attempt = attempt_for_step(&run, &consumer_step_id);
    assert_eq!(producer_attempt.status, "succeeded", "{run}");
    assert_eq!(consumer_attempt.status, "succeeded", "{run}");

    let inputs_json = fs::read_to_string(consumer_attempt.workdir.join("inputs.json"))
        .expect("consumer inputs.json should exist");
    let staged_input = PathBuf::from(json_string_field(&inputs_json, "upstream_report"));
    let expected_input_root = consumer_attempt
        .workdir
        .join("inputs")
        .join("upstream_report");
    assert!(
        staged_input.starts_with(&expected_input_root),
        "expected staged input under {}, got {} from {inputs_json}",
        expected_input_root.display(),
        staged_input.display()
    );
    assert!(
        !staged_input
            .to_string_lossy()
            .contains(".agentflow/artifacts/"),
        "consumer input should be staged, not an artifact-store path: {}",
        staged_input.display()
    );

    let artifacts = run_agentflow(["artifacts", "list", "--json", "--path", project_arg]);
    let producer_artifact = computed_artifact_path_for_source(&artifacts, &producer_step_id)
        .expect("expected computed artifact from producer step");
    let producer_artifact = project_artifact_path(&project.path, producer_artifact);
    assert!(
        producer_artifact.exists(),
        "producer computed artifact should exist at {} from {artifacts}",
        producer_artifact.display()
    );
}
