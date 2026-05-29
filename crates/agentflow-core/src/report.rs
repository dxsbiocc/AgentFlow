use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::path::PathBuf;

use rusqlite::params;

use crate::storage::{ArtifactSummary, ProjectStore, StorageError, StoredFlowStep};

#[derive(Debug, Clone, PartialEq, Eq)]
struct RunAttemptRecord {
    run_id: String,
    step_id: String,
    step_local_id: String,
    attempt_id: String,
    attempt_number: i64,
    status: String,
    workdir: Option<PathBuf>,
    started_at: Option<i64>,
    ended_at: Option<i64>,
    exit_code: Option<i32>,
    stdout_path: Option<PathBuf>,
    stderr_path: Option<PathBuf>,
    error_class: Option<String>,
    error_message: Option<String>,
}

impl RunAttemptRecord {
    fn failed(&self) -> bool {
        !matches!(self.status.as_str(), "succeeded" | "cache_hit")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReferencedArtifactRecord {
    step_id: String,
    step_local_id: String,
    input_name: String,
    input_value: String,
    summary: ArtifactSummary,
}

impl ProjectStore {
    pub fn generate_report_markdown(&self, flow_id: &str) -> Result<String, StorageError> {
        let flow = self.inspect_flow(flow_id)?;
        let step_local_ids = flow
            .steps
            .iter()
            .map(|step| (step.id.clone(), step.local_id.clone()))
            .collect::<BTreeMap<_, _>>();
        let attempts = load_run_attempts(self, flow_id, &step_local_ids)?;
        let produced_artifacts = load_produced_artifacts(self, &step_local_ids)?;
        let referenced_artifacts = load_referenced_artifacts(self, &flow.steps)?;
        let run_count = self.connection().query_row(
            "SELECT COUNT(*) FROM runs WHERE flow_id = ?1",
            params![flow_id],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let failed_attempts = attempts
            .iter()
            .filter(|attempt| attempt.failed())
            .collect::<Vec<_>>();
        let referenced_artifact_ids = referenced_artifacts
            .iter()
            .map(|artifact| artifact.summary.id.as_str())
            .collect::<BTreeSet<_>>();

        let mut attempts_by_step = BTreeMap::<&str, Vec<&RunAttemptRecord>>::new();
        for attempt in &attempts {
            attempts_by_step
                .entry(attempt.step_id.as_str())
                .or_default()
                .push(attempt);
        }

        let mut produced_by_step = BTreeMap::<&str, Vec<&ArtifactSummary>>::new();
        for artifact in &produced_artifacts {
            if let Some(step_id) = artifact.source_step_id.as_deref() {
                produced_by_step.entry(step_id).or_default().push(artifact);
            }
        }

        let mut referenced_by_step = BTreeMap::<&str, Vec<&ReferencedArtifactRecord>>::new();
        for artifact in &referenced_artifacts {
            referenced_by_step
                .entry(artifact.step_id.as_str())
                .or_default()
                .push(artifact);
        }

        let mut markdown = String::new();
        writeln!(&mut markdown, "# Flow Report: {}", flow.name).unwrap();
        writeln!(&mut markdown).unwrap();
        writeln!(&mut markdown, "- Flow ID: `{}`", flow.id).unwrap();
        writeln!(&mut markdown, "- Status: `{}`", flow.status).unwrap();
        writeln!(&mut markdown, "- Schema Version: `{}`", flow.schema_version).unwrap();
        writeln!(
            &mut markdown,
            "- Source Path: {}",
            flow.source_path
                .as_ref()
                .map(|path| format!("`{}`", path.display()))
                .unwrap_or_else(|| "_none_".to_string())
        )
        .unwrap();
        writeln!(&mut markdown, "- Created At: `{}`", flow.created_at).unwrap();
        writeln!(&mut markdown, "- Updated At: `{}`", flow.updated_at).unwrap();
        writeln!(&mut markdown, "- Steps: `{}`", flow.steps.len()).unwrap();
        writeln!(&mut markdown, "- Runs: `{}`", run_count).unwrap();
        writeln!(&mut markdown, "- Attempts: `{}`", attempts.len()).unwrap();
        writeln!(
            &mut markdown,
            "- Referenced Input Artifacts: `{}`",
            referenced_artifact_ids.len()
        )
        .unwrap();
        writeln!(
            &mut markdown,
            "- Produced Artifacts: `{}`",
            produced_artifacts.len()
        )
        .unwrap();
        writeln!(
            &mut markdown,
            "- Failed Attempts: `{}`",
            failed_attempts.len()
        )
        .unwrap();
        writeln!(&mut markdown).unwrap();

        writeln!(&mut markdown, "## Steps").unwrap();
        writeln!(&mut markdown).unwrap();
        for (index, step) in flow.steps.iter().enumerate() {
            let inputs = parse_json_map(&step.inputs_json)?;
            let params = parse_json_map(&step.params_json)?;
            let outputs = parse_json_map(&step.outputs_json)?;

            writeln!(&mut markdown, "### {}. `{}`", index + 1, step.local_id).unwrap();
            writeln!(&mut markdown).unwrap();
            writeln!(&mut markdown, "- Step Record ID: `{}`", step.id).unwrap();
            writeln!(
                &mut markdown,
                "- Tool: {}",
                step.tool_ref
                    .as_deref()
                    .map(|tool| format!("`{tool}`"))
                    .unwrap_or_else(|| "_none_".to_string())
            )
            .unwrap();
            writeln!(&mut markdown, "- Type: `{}`", step.step_type).unwrap();
            writeln!(&mut markdown, "- Status: `{}`", step.status).unwrap();
            writeln!(
                &mut markdown,
                "- Reason: {}",
                step.reason
                    .as_deref()
                    .map(|reason| format!("`{reason}`"))
                    .unwrap_or_else(|| "_none_".to_string())
            )
            .unwrap();

            writeln!(&mut markdown, "- Inputs:").unwrap();
            if inputs.is_empty() {
                writeln!(&mut markdown, "  - _none_").unwrap();
            } else {
                for (name, value) in inputs {
                    writeln!(&mut markdown, "  - `{name}`: `{value}`").unwrap();
                }
            }

            writeln!(&mut markdown, "- Params:").unwrap();
            if params.is_empty() {
                writeln!(&mut markdown, "  - _none_").unwrap();
            } else {
                for (name, value) in params {
                    writeln!(&mut markdown, "  - `{name}`: `{value}`").unwrap();
                }
            }

            writeln!(&mut markdown, "- Declared Outputs:").unwrap();
            if outputs.is_empty() {
                writeln!(&mut markdown, "  - _none_").unwrap();
            } else {
                for (name, value) in outputs {
                    writeln!(&mut markdown, "  - `{name}`: `{value}`").unwrap();
                }
            }

            writeln!(&mut markdown, "- Attempts:").unwrap();
            if let Some(step_attempts) = attempts_by_step.get(step.id.as_str()) {
                for attempt in step_attempts {
                    writeln!(
                        &mut markdown,
                        "  - attempt {} / `{}`: status `{}`, exit `{}`, started `{}`, ended `{}`",
                        attempt.attempt_number,
                        attempt.attempt_id,
                        attempt.status,
                        format_optional_i32(attempt.exit_code),
                        format_optional_i64(attempt.started_at),
                        format_optional_i64(attempt.ended_at)
                    )
                    .unwrap();
                    writeln!(
                        &mut markdown,
                        "    - run `{}`, workdir {}, stdout {}, stderr {}",
                        attempt.run_id,
                        format_optional_path(attempt.workdir.as_ref()),
                        format_optional_path(attempt.stdout_path.as_ref()),
                        format_optional_path(attempt.stderr_path.as_ref())
                    )
                    .unwrap();
                    if attempt.failed() {
                        writeln!(
                            &mut markdown,
                            "    - failure: class {}, message {}",
                            format_optional_text(attempt.error_class.as_deref()),
                            format_optional_text(attempt.error_message.as_deref())
                        )
                        .unwrap();
                    }
                }
            } else {
                writeln!(&mut markdown, "  - _none_").unwrap();
            }

            writeln!(&mut markdown, "- Referenced Input Artifacts:").unwrap();
            if let Some(step_artifacts) = referenced_by_step.get(step.id.as_str()) {
                for artifact in step_artifacts {
                    writeln!(
                        &mut markdown,
                        "  - `{}` via input `{}` (`{}`): kind `{}`, type `{}`, path `{}`",
                        artifact.summary.id,
                        artifact.input_name,
                        artifact.input_value,
                        artifact.summary.kind,
                        artifact.summary.artifact_type,
                        artifact.summary.path.display()
                    )
                    .unwrap();
                }
            } else {
                writeln!(&mut markdown, "  - _none_").unwrap();
            }

            writeln!(&mut markdown, "- Produced Artifacts:").unwrap();
            if let Some(step_artifacts) = produced_by_step.get(step.id.as_str()) {
                for artifact in step_artifacts {
                    writeln!(
                        &mut markdown,
                        "  - `{}`: kind `{}`, type `{}`, path `{}`, size `{}`",
                        artifact.id,
                        artifact.kind,
                        artifact.artifact_type,
                        artifact.path.display(),
                        format_optional_i64(artifact.size_bytes)
                    )
                    .unwrap();
                }
            } else {
                writeln!(&mut markdown, "  - _none_").unwrap();
            }

            writeln!(&mut markdown).unwrap();
        }

        writeln!(&mut markdown, "## Attempts").unwrap();
        writeln!(&mut markdown).unwrap();
        if attempts.is_empty() {
            writeln!(&mut markdown, "- _none_").unwrap();
        } else {
            for attempt in &attempts {
                writeln!(
                    &mut markdown,
                    "- `{}` / step `{}` / attempt {}: status `{}`, exit `{}`, error {}",
                    attempt.run_id,
                    attempt.step_local_id,
                    attempt.attempt_number,
                    attempt.status,
                    format_optional_i32(attempt.exit_code),
                    format_optional_text(attempt.error_message.as_deref())
                )
                .unwrap();
            }
        }
        writeln!(&mut markdown).unwrap();

        writeln!(&mut markdown, "## Artifacts").unwrap();
        writeln!(&mut markdown).unwrap();
        writeln!(&mut markdown, "### Referenced Inputs").unwrap();
        writeln!(&mut markdown).unwrap();
        if referenced_artifacts.is_empty() {
            writeln!(&mut markdown, "- _none_").unwrap();
        } else {
            for artifact in &referenced_artifacts {
                writeln!(
                    &mut markdown,
                    "- `{}` / step `{}` / input `{}`: kind `{}`, type `{}`, path `{}`",
                    artifact.summary.id,
                    artifact.step_local_id,
                    artifact.input_name,
                    artifact.summary.kind,
                    artifact.summary.artifact_type,
                    artifact.summary.path.display()
                )
                .unwrap();
            }
        }
        writeln!(&mut markdown).unwrap();

        writeln!(&mut markdown, "### Produced Outputs").unwrap();
        writeln!(&mut markdown).unwrap();
        if produced_artifacts.is_empty() {
            writeln!(&mut markdown, "- _none_").unwrap();
        } else {
            for artifact in &produced_artifacts {
                writeln!(
                    &mut markdown,
                    "- `{}` / step `{}` / run {}: kind `{}`, type `{}`, path `{}`",
                    artifact.id,
                    artifact
                        .source_step_id
                        .as_deref()
                        .and_then(|step_id| step_local_ids.get(step_id))
                        .map(|step_id| format!("`{step_id}`"))
                        .unwrap_or_else(|| "_unknown_".to_string()),
                    artifact
                        .source_run_id
                        .as_deref()
                        .map(|run_id| format!("`{run_id}`"))
                        .unwrap_or_else(|| "_none_".to_string()),
                    artifact.kind,
                    artifact.artifact_type,
                    artifact.path.display()
                )
                .unwrap();
            }
        }
        writeln!(&mut markdown).unwrap();

        let observations = self.list_observations()?;
        let graph_patches = self.list_graph_patches(flow_id)?;
        let branch_comparisons = self.list_branch_comparisons(flow_id)?;
        let research_notes = self.list_research_notes()?;

        writeln!(&mut markdown, "## Evidence State").unwrap();
        writeln!(&mut markdown).unwrap();

        writeln!(&mut markdown, "### Observations").unwrap();
        writeln!(&mut markdown).unwrap();
        let mut observation_count = 0;
        for observation in &observations {
            if !observation_belongs_to_flow(
                observation.flow_id.as_deref(),
                observation.step_id.as_deref(),
                observation.artifact_id.as_deref(),
                flow_id,
                &step_local_ids,
                &referenced_artifact_ids,
            ) {
                continue;
            }
            observation_count += 1;
            writeln!(
                &mut markdown,
                "- `{}` / kind `{}` / severity `{}` / artifact {}: {}",
                observation.id,
                observation.kind,
                observation.severity,
                format_optional_text(observation.artifact_id.as_deref()),
                observation.summary
            )
            .unwrap();
        }
        if observation_count == 0 {
            writeln!(&mut markdown, "- _none_").unwrap();
        }
        writeln!(&mut markdown).unwrap();

        writeln!(&mut markdown, "### Graph Patches").unwrap();
        writeln!(&mut markdown).unwrap();
        if graph_patches.is_empty() {
            writeln!(&mut markdown, "- _none_").unwrap();
        } else {
            for patch in &graph_patches {
                writeln!(
                    &mut markdown,
                    "- `{}` / status `{}`: {}\n  - reason: {}\n  - decision reason: {}",
                    patch.id,
                    patch.status,
                    patch.title,
                    patch.reason,
                    format_optional_text(patch.decision_reason.as_deref())
                )
                .unwrap();
            }
        }
        writeln!(&mut markdown).unwrap();

        writeln!(&mut markdown, "### Branch Comparisons").unwrap();
        writeln!(&mut markdown).unwrap();
        if branch_comparisons.is_empty() {
            writeln!(&mut markdown, "- _none_").unwrap();
        } else {
            for comparison in &branch_comparisons {
                writeln!(
                    &mut markdown,
                    "- `{}` / baseline `{}` / candidate `{}` / winner {}: {}\n  - reason: {}",
                    comparison.id,
                    comparison.baseline_step,
                    comparison.candidate_step,
                    format_optional_text(comparison.winner.as_deref()),
                    comparison.summary,
                    format_optional_text(comparison.reason.as_deref())
                )
                .unwrap();
            }
        }
        writeln!(&mut markdown).unwrap();

        writeln!(&mut markdown, "### Research Notes").unwrap();
        writeln!(&mut markdown).unwrap();
        if research_notes.is_empty() {
            writeln!(&mut markdown, "- _none_").unwrap();
        } else {
            for note in &research_notes {
                writeln!(
                    &mut markdown,
                    "- `{}` / confidence `{}`: {}\n  - problem: {}\n  - finding: {}\n  - source: {}",
                    note.id,
                    note.confidence,
                    note.question,
                    note.problem,
                    note.finding,
                    format_optional_text(note.source.as_deref())
                )
                .unwrap();
            }
        }
        writeln!(&mut markdown).unwrap();

        writeln!(&mut markdown, "## Failures").unwrap();
        writeln!(&mut markdown).unwrap();
        if failed_attempts.is_empty() {
            writeln!(&mut markdown, "- _none_").unwrap();
        } else {
            for attempt in failed_attempts {
                writeln!(
                    &mut markdown,
                    "- step `{}` / attempt `{}`: status `{}`, exit `{}`, class {}, message {}",
                    attempt.step_local_id,
                    attempt.attempt_id,
                    attempt.status,
                    format_optional_i32(attempt.exit_code),
                    format_optional_text(attempt.error_class.as_deref()),
                    format_optional_text(attempt.error_message.as_deref())
                )
                .unwrap();
            }
        }

        Ok(markdown)
    }
}

fn load_run_attempts(
    store: &ProjectStore,
    flow_id: &str,
    step_local_ids: &BTreeMap<String, String>,
) -> Result<Vec<RunAttemptRecord>, StorageError> {
    let mut stmt = store.connection().prepare(
        "SELECT runs.id,
                runs.step_id,
                run_attempts.id,
                run_attempts.attempt,
                run_attempts.status,
                run_attempts.workdir,
                run_attempts.started_at,
                run_attempts.ended_at,
                run_attempts.exit_code,
                run_attempts.stdout_path,
                run_attempts.stderr_path,
                run_attempts.error_class,
                run_attempts.error_message
         FROM runs
         INNER JOIN run_attempts ON run_attempts.run_id = runs.id
         WHERE runs.flow_id = ?1
         ORDER BY runs.created_at ASC, run_attempts.attempt ASC, run_attempts.id ASC",
    )?;
    let rows = stmt.query_map(params![flow_id], |row| {
        let step_id = row.get::<_, String>(1)?;
        Ok(RunAttemptRecord {
            run_id: row.get(0)?,
            step_local_id: step_local_ids
                .get(&step_id)
                .cloned()
                .unwrap_or_else(|| step_id.clone()),
            step_id,
            attempt_id: row.get(2)?,
            attempt_number: row.get(3)?,
            status: row.get(4)?,
            workdir: row.get::<_, Option<String>>(5)?.map(PathBuf::from),
            started_at: row.get(6)?,
            ended_at: row.get(7)?,
            exit_code: row.get(8)?,
            stdout_path: row.get::<_, Option<String>>(9)?.map(PathBuf::from),
            stderr_path: row.get::<_, Option<String>>(10)?.map(PathBuf::from),
            error_class: row.get(11)?,
            error_message: row.get(12)?,
        })
    })?;

    let mut attempts = Vec::new();
    for row in rows {
        attempts.push(row?);
    }
    Ok(attempts)
}

fn load_produced_artifacts(
    store: &ProjectStore,
    step_local_ids: &BTreeMap<String, String>,
) -> Result<Vec<ArtifactSummary>, StorageError> {
    let step_ids = step_local_ids.keys().collect::<BTreeSet<_>>();
    Ok(store
        .list_artifacts()?
        .into_iter()
        .filter(|artifact| {
            artifact
                .source_step_id
                .as_ref()
                .is_some_and(|step_id| step_ids.contains(step_id))
        })
        .collect())
}

fn load_referenced_artifacts(
    store: &ProjectStore,
    steps: &[StoredFlowStep],
) -> Result<Vec<ReferencedArtifactRecord>, StorageError> {
    let mut seen = BTreeSet::new();
    let mut referenced = Vec::new();

    for step in steps {
        let inputs = parse_json_map(&step.inputs_json)?;
        for (input_name, input_value) in inputs {
            let Some(artifact_id) = artifact_ref(&input_value) else {
                continue;
            };
            let dedupe_key = (step.id.clone(), input_name.clone(), artifact_id.to_string());
            if !seen.insert(dedupe_key) {
                continue;
            }

            let artifact = store.inspect_artifact(artifact_id)?;
            referenced.push(ReferencedArtifactRecord {
                step_id: step.id.clone(),
                step_local_id: step.local_id.clone(),
                input_name,
                input_value,
                summary: artifact.summary,
            });
        }
    }

    Ok(referenced)
}

fn artifact_ref(value: &str) -> Option<&str> {
    let artifact_id = value.strip_prefix("artifact:").unwrap_or(value);
    if artifact_id.contains('.') {
        return None;
    }
    artifact_id.starts_with("artifact_").then_some(artifact_id)
}

fn observation_belongs_to_flow(
    observation_flow_id: Option<&str>,
    observation_step_id: Option<&str>,
    observation_artifact_id: Option<&str>,
    flow_id: &str,
    step_local_ids: &BTreeMap<String, String>,
    referenced_artifact_ids: &BTreeSet<&str>,
) -> bool {
    if observation_flow_id == Some(flow_id) {
        return true;
    }
    if observation_step_id.is_some_and(|step_id| step_local_ids.contains_key(step_id)) {
        return true;
    }
    observation_artifact_id.is_some_and(|artifact_id| referenced_artifact_ids.contains(artifact_id))
}

fn parse_json_map(input: &str) -> Result<BTreeMap<String, String>, StorageError> {
    let mut map = BTreeMap::new();
    let mut index = 0;
    skip_json_whitespace(input, &mut index);
    expect_json_char(input, &mut index, '{')?;
    skip_json_whitespace(input, &mut index);
    if consume_json_char(input, &mut index, '}') {
        return Ok(map);
    }

    loop {
        let key = parse_json_string(input, &mut index)?;
        skip_json_whitespace(input, &mut index);
        expect_json_char(input, &mut index, ':')?;
        skip_json_whitespace(input, &mut index);
        let value = parse_json_string(input, &mut index)?;
        map.insert(key, value);
        skip_json_whitespace(input, &mut index);
        if consume_json_char(input, &mut index, ',') {
            skip_json_whitespace(input, &mut index);
            continue;
        }
        if consume_json_char(input, &mut index, '}') {
            break;
        }
        return Err(StorageError::InvalidInput(format!(
            "cannot parse report map: {input}"
        )));
    }

    skip_json_whitespace(input, &mut index);
    if index != input.len() {
        return Err(StorageError::InvalidInput(format!(
            "cannot parse report map: {input}"
        )));
    }
    Ok(map)
}

fn parse_json_string(input: &str, index: &mut usize) -> Result<String, StorageError> {
    expect_json_char(input, index, '"')?;
    let rest = input.get(*index..).ok_or_else(|| {
        StorageError::InvalidInput(format!("cannot parse json string in report map: {input}"))
    })?;
    let end = find_json_string_end(rest)
        .ok_or_else(|| StorageError::InvalidInput(format!("cannot parse report map: {input}")))?;
    let value = unescape_json_string(&rest[..end]);
    *index += end + 1;
    Ok(value)
}

fn expect_json_char(input: &str, index: &mut usize, expected: char) -> Result<(), StorageError> {
    if consume_json_char(input, index, expected) {
        Ok(())
    } else {
        Err(StorageError::InvalidInput(format!(
            "expected '{expected}' while parsing report map: {input}"
        )))
    }
}

fn consume_json_char(input: &str, index: &mut usize, expected: char) -> bool {
    if input
        .get(*index..)
        .and_then(|rest| rest.chars().next())
        .is_some_and(|actual| actual == expected)
    {
        *index += expected.len_utf8();
        true
    } else {
        false
    }
}

fn skip_json_whitespace(input: &str, index: &mut usize) {
    while input
        .get(*index..)
        .and_then(|rest| rest.chars().next())
        .is_some_and(char::is_whitespace)
    {
        let ch = input[*index..].chars().next().expect("checked above");
        *index += ch.len_utf8();
    }
}

fn find_json_string_end(input: &str) -> Option<usize> {
    let mut escaped = false;
    for (index, ch) in input.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Some(index),
            _ => {}
        }
    }
    None
}

fn unescape_json_string(input: &str) -> String {
    let mut output = String::new();
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('"') => output.push('"'),
                Some('\\') => output.push('\\'),
                Some('n') => output.push('\n'),
                Some('r') => output.push('\r'),
                Some('t') => output.push('\t'),
                Some(other) => output.push(other),
                None => {}
            }
        } else {
            output.push(ch);
        }
    }
    output
}

fn format_optional_i32(value: Option<i32>) -> String {
    value
        .map(|value| format!("`{value}`"))
        .unwrap_or_else(|| "_none_".to_string())
}

fn format_optional_i64(value: Option<i64>) -> String {
    value
        .map(|value| format!("`{value}`"))
        .unwrap_or_else(|| "_none_".to_string())
}

fn format_optional_path(value: Option<&PathBuf>) -> String {
    value
        .map(|value| format!("`{}`", value.display()))
        .unwrap_or_else(|| "_none_".to_string())
}

fn format_optional_text(value: Option<&str>) -> String {
    value
        .map(|value| format!("`{value}`"))
        .unwrap_or_else(|| "_none_".to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::storage::{ArtifactImportMode, ArtifactImportRequest, FlowDraft, ToolSpec};
    use crate::{comparison::BranchComparisonRequest, research::ResearchNoteRequest};

    fn temp_project_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentflow-report-{test_name}-{}-{}",
            std::process::id(),
            crate::storage::now_unix_seconds()
        ))
    }

    fn write_runtime_script(path: &Path) -> PathBuf {
        let script_path = path.join("report_tool.sh");
        fs::write(
            &script_path,
            r#"if [ -n "$AGENTFLOW_OUTPUT_MARKER_REPORT" ]; then
  printf '# Marker report\nGene: %s\n' "$AGENTFLOW_PARAM_GENE" > "$AGENTFLOW_OUTPUT_MARKER_REPORT"
  echo "scan ok"
fi
if [ -n "$AGENTFLOW_OUTPUT_FINAL_REPORT" ]; then
  cat "$AGENTFLOW_INPUT_UPSTREAM_REPORT" > "$AGENTFLOW_OUTPUT_FINAL_REPORT"
  printf '\nfinalized\n' >> "$AGENTFLOW_OUTPUT_FINAL_REPORT"
  echo "finalize ok"
fi
"#,
        )
        .unwrap();
        script_path
    }

    fn register_tool(store: &ProjectStore, source: String) {
        store
            .register_tool(ToolSpec::from_simple_yaml(&source).unwrap())
            .unwrap();
    }

    fn import_artifact(store: &ProjectStore, source_path: PathBuf) -> String {
        store
            .import_artifact(ArtifactImportRequest {
                source_path,
                artifact_type: "TSV".to_string(),
                mode: ArtifactImportMode::Reference,
            })
            .unwrap()
            .summary
            .id
    }

    #[test]
    fn report_json_map_parser_handles_punctuation_inside_strings() {
        let parsed =
            parse_json_map(r#"{"gene":"TP53,EGFR:ALK","label":"quoted \"value\""}"#).unwrap();
        assert_eq!(parsed["gene"], "TP53,EGFR:ALK");
        assert_eq!(parsed["label"], "quoted \"value\"");
    }

    #[test]
    fn generate_report_markdown_covers_flow_steps_attempts_and_artifacts() {
        let path = temp_project_path("success");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Report Demo")).unwrap();
        let script_path = write_runtime_script(&path);
        let command = script_path.display();

        register_tool(
            &store,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: marker
name: marker_survival_scan
version: 0.1.0
maturity: wrapped
description: Scan a candidate marker against survival table
inputs:
  expression_table:
    type: TSV
    required: true
  survival_table:
    type: TSV
    required: true
params:
  gene:
    type: string
    required: true
outputs:
  marker_report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/sh
    - {command}
"#
            ),
        );
        register_tool(
            &store,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: report
name: finalize_report
version: 0.1.0
maturity: wrapped
description: Finalize an upstream report
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
    - {command}
"#
            ),
        );

        let expression_path = path.join("expression.tsv");
        fs::write(&expression_path, "sample\tTP53\nA\t1.2\n").unwrap();
        let survival_path = path.join("survival.tsv");
        fs::write(&survival_path, "sample\ttime\tstatus\nA\t10\t1\n").unwrap();
        let expression_id = import_artifact(&store, expression_path);
        let survival_id = import_artifact(&store, survival_path);

        let flow = FlowDraft::from_simple_yaml(&format!(
            r#"
schema_version: agentflow.flow.v0
id: marker_demo
name: Marker demo
steps:
  - id: artifact_scan
    tool: marker/marker_survival_scan
    reason: Evaluate TP53 marker signal
    needs: []
    inputs:
      expression_table: {expression_id}
      survival_table: {survival_id}
    params:
      gene: TP53
    outputs:
      marker_report: marker_report
  - id: finalize
    tool: report/finalize_report
    reason: Prepare final report artifact
    needs: [artifact_scan]
    inputs:
      upstream_report: artifact_scan.marker_report
    outputs:
      final_report: final_report
"#
        ))
        .unwrap();
        store.approve_flow(flow, None).unwrap();
        store.run_flow("marker_demo").unwrap();

        let markdown = store.generate_report_markdown("marker_demo").unwrap();
        assert!(markdown.contains("# Flow Report: Marker demo"));
        assert!(markdown.contains("## Steps"));
        assert!(markdown.contains("`artifact_scan`"));
        assert!(markdown.contains("`finalize`"));
        assert!(markdown.contains("## Attempts"));
        assert!(markdown.contains("status `succeeded`"));
        assert!(markdown.contains("## Artifacts"));
        assert!(markdown.contains("Referenced Inputs"));
        assert!(markdown.contains("Produced Outputs"));
        assert!(markdown.contains(&expression_id));
        assert!(markdown.contains("marker_report"));
        assert!(markdown.contains("final_report"));
        assert!(markdown.contains("## Failures\n\n- _none_"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn generate_report_markdown_includes_observations_patches_and_research_notes() {
        let path = temp_project_path("evidence-state");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Report Evidence")).unwrap();
        let script_path = write_runtime_script(&path);
        let command = script_path.display();

        register_tool(
            &store,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: marker
name: marker_survival_scan
version: 0.1.0
maturity: wrapped
description: Scan a candidate marker against survival table
inputs:
  expression_table:
    type: TSV
    required: true
  survival_table:
    type: TSV
    required: true
params:
  gene:
    type: string
    required: true
outputs:
  marker_report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/sh
    - {command}
"#
            ),
        );

        let expression_path = path.join("expression.tsv");
        fs::write(&expression_path, "sample\tTP53\nA\t1.2\n").unwrap();
        let expression_id = import_artifact(&store, expression_path);
        let survival_path = path.join("survival.tsv");
        fs::write(&survival_path, "sample\ttime\tstatus\nA\t10\t1\n").unwrap();
        let survival_id = import_artifact(&store, survival_path);

        store
            .approve_flow(
                FlowDraft::from_simple_yaml(&format!(
                    r#"
schema_version: agentflow.flow.v0
id: evidence_demo
name: Evidence demo
steps:
  - id: scan
    tool: marker/marker_survival_scan
    reason: Evaluate marker signal
    needs: []
    inputs:
      expression_table: {expression_id}
      survival_table: {survival_id}
    params:
      gene: TP53
    outputs:
      marker_report: marker_report
  - id: ortholog_scan
    tool: marker/marker_survival_scan
    reason: Evaluate related marker signal
    needs: []
    inputs:
      expression_table: {expression_id}
      survival_table: {survival_id}
    params:
      gene: EGFR
    outputs:
      marker_report: ortholog_report
"#
                ))
                .unwrap(),
                None,
            )
            .unwrap();

        let observation = store.observe_artifact(&expression_id).unwrap();
        assert!(observation.summary.contains("expression.tsv"));

        let patch = store
            .propose_graph_patch(
                "evidence_demo",
                "Add ortholog branch",
                "Primary marker is weak; compare related gene.",
                r#"{"ops":[{"op":"add_edge","from":"scan","to":"ortholog_scan"}]}"#,
            )
            .unwrap();
        store
            .reject_graph_patch(&patch.id, "Branch lacks a registered target step.")
            .unwrap();
        let patch = store
            .list_graph_patches("evidence_demo")
            .unwrap()
            .into_iter()
            .find(|patch| patch.title == "Add ortholog branch")
            .unwrap();
        assert_eq!(patch.status, "rejected");

        let comparison = store
            .record_branch_comparison(BranchComparisonRequest {
                flow_id: "evidence_demo".to_string(),
                baseline_step: "scan".to_string(),
                candidate_step: "ortholog_scan".to_string(),
                summary: "Ortholog branch is plausible but not decisive.".to_string(),
                winner: Some("inconclusive".to_string()),
                reason: Some("Needs external validation.".to_string()),
            })
            .unwrap();

        let note = store
            .record_research_note(ResearchNoteRequest {
                problem: "Marker failed validation".to_string(),
                question: "Should homolog genes be considered?".to_string(),
                finding: "A homolog remains plausible in the pathway.".to_string(),
                confidence: "medium".to_string(),
                source: Some("local review".to_string()),
            })
            .unwrap();

        let report = store.generate_report_markdown("evidence_demo").unwrap();
        assert!(report.contains("## Evidence State"));
        assert!(report.contains("### Observations"));
        assert!(report.contains(&observation.id));
        assert!(report.contains("### Graph Patches"));
        assert!(report.contains("Add ortholog branch"));
        assert!(report.contains("status `rejected`"));
        assert!(report.contains("### Branch Comparisons"));
        assert!(report.contains(&comparison.id));
        assert!(report.contains("Ortholog branch is plausible"));
        assert!(report.contains("### Research Notes"));
        assert!(report.contains(&note.id));
        assert!(report.contains("confidence `medium`"));
        assert!(report.contains("A homolog remains plausible"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn generate_report_markdown_includes_failed_attempt_details() {
        let path = temp_project_path("failure");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Report Failure Demo")).unwrap();
        let script_path = path.join("fail_tool.sh");
        fs::write(
            &script_path,
            r#"echo "failure stdout"
echo "boom" >&2
exit 3
"#,
        )
        .unwrap();

        register_tool(
            &store,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: marker
name: failing_scan
version: 0.1.0
maturity: wrapped
description: Fail deliberately
inputs:
  expression_table:
    type: TSV
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
                script_path.display()
            ),
        );

        let expression_path = path.join("expression.tsv");
        fs::write(&expression_path, "sample\tTP53\nA\t1.2\n").unwrap();
        let expression_id = import_artifact(&store, expression_path);
        let flow = FlowDraft::from_simple_yaml(&format!(
            r#"
schema_version: agentflow.flow.v0
id: failing_demo
name: Failing demo
steps:
  - id: scan
    tool: marker/failing_scan
    reason: Prove failed attempts retain logs
    needs: []
    inputs:
      expression_table: {expression_id}
    outputs:
      report: marker_report
"#
        ))
        .unwrap();
        store.approve_flow(flow, None).unwrap();
        store.run_flow("failing_demo").unwrap();

        let markdown = store.generate_report_markdown("failing_demo").unwrap();
        assert!(markdown.contains("# Flow Report: Failing demo"));
        assert!(markdown.contains("status `failed`"));
        assert!(markdown.contains("message `command exited with code Some(3)`"));
        assert!(markdown.contains("## Failures"));
        assert!(markdown.contains("step `scan` / attempt `attempt_"));

        let _ = fs::remove_dir_all(path);
    }
}
