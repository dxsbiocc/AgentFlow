use rusqlite::params;

use crate::observer::normalize_metric_name;
use crate::storage::{ArtifactSummary, EventRecord, ProjectStore, StorageError};

const BRANCH_COMPARISON_EVENT: &str = "branch_comparison_recorded";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchComparisonRequest {
    pub flow_id: String,
    pub baseline_step: String,
    pub candidate_step: String,
    pub summary: String,
    pub winner: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchComparisonRecord {
    pub id: String,
    pub flow_id: String,
    pub baseline_step: String,
    pub candidate_step: String,
    pub summary: String,
    pub winner: Option<String>,
    pub reason: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricComparisonRequest {
    pub flow_id: String,
    pub baseline_step: String,
    pub candidate_step: String,
    pub metric: String,
    pub direction: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MetricComparisonResult {
    pub comparison: BranchComparisonRecord,
    pub metric: String,
    pub direction: String,
    pub baseline_value: f64,
    pub candidate_value: f64,
    pub baseline_artifact_id: String,
    pub candidate_artifact_id: String,
}

impl MetricComparisonResult {
    pub fn to_json(&self) -> String {
        format!(
            concat!(
                "{{",
                "\"comparison\":{},",
                "\"metric\":\"{}\",",
                "\"direction\":\"{}\",",
                "\"baseline_value\":{},",
                "\"candidate_value\":{},",
                "\"baseline_artifact_id\":\"{}\",",
                "\"candidate_artifact_id\":\"{}\"",
                "}}"
            ),
            self.comparison.to_json(),
            escape_json(&self.metric),
            escape_json(&self.direction),
            format_metric_value(self.baseline_value),
            format_metric_value(self.candidate_value),
            escape_json(&self.baseline_artifact_id),
            escape_json(&self.candidate_artifact_id)
        )
    }
}

impl BranchComparisonRecord {
    pub fn to_json(&self) -> String {
        format!(
            concat!(
                "{{",
                "\"id\":\"{}\",",
                "\"flow_id\":\"{}\",",
                "\"baseline_step\":\"{}\",",
                "\"candidate_step\":\"{}\",",
                "\"summary\":\"{}\",",
                "\"winner\":{},",
                "\"reason\":{},",
                "\"created_at\":{}",
                "}}"
            ),
            escape_json(&self.id),
            escape_json(&self.flow_id),
            escape_json(&self.baseline_step),
            escape_json(&self.candidate_step),
            escape_json(&self.summary),
            optional_json_string(self.winner.as_deref()),
            optional_json_string(self.reason.as_deref()),
            self.created_at
        )
    }
}

impl ProjectStore {
    pub fn record_branch_comparison(
        &self,
        request: BranchComparisonRequest,
    ) -> Result<BranchComparisonRecord, StorageError> {
        validate_request(self, &request)?;
        let id = self.append_event(EventRecord {
            flow_id: Some(request.flow_id.clone()),
            step_id: None,
            run_id: None,
            event_type: BRANCH_COMPARISON_EVENT.to_string(),
            payload_json: comparison_payload_json(&request),
        })?;
        self.touch_project()?;
        self.inspect_branch_comparison(&id)
    }

    pub fn list_branch_comparisons(
        &self,
        flow_id: &str,
    ) -> Result<Vec<BranchComparisonRecord>, StorageError> {
        if flow_id.trim().is_empty() {
            return Err(StorageError::InvalidInput(
                "flow id must not be empty".to_string(),
            ));
        }
        self.inspect_flow(flow_id)?;

        let mut stmt = self.connection().prepare(
            "SELECT id, flow_id, payload_json, created_at
             FROM events
             WHERE flow_id = ?1 AND event_type = ?2
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map(params![flow_id, BRANCH_COMPARISON_EVENT], |row| {
            let id = row.get::<_, String>(0)?;
            let flow_id = row.get::<_, Option<String>>(1)?.ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Null,
                    "branch comparison missing flow_id".into(),
                )
            })?;
            let payload_json = row.get::<_, String>(2)?;
            let created_at = row.get::<_, i64>(3)?;
            comparison_from_event(id, flow_id, &payload_json, created_at)
        })?;

        let mut comparisons = Vec::new();
        for row in rows {
            comparisons.push(row?);
        }
        Ok(comparisons)
    }

    pub fn inspect_branch_comparison(
        &self,
        comparison_id: &str,
    ) -> Result<BranchComparisonRecord, StorageError> {
        if comparison_id.trim().is_empty() {
            return Err(StorageError::InvalidInput(
                "branch comparison id must not be empty".to_string(),
            ));
        }

        self.connection()
            .query_row(
                "SELECT id, flow_id, payload_json, created_at
                 FROM events
                 WHERE id = ?1 AND event_type = ?2",
                params![comparison_id, BRANCH_COMPARISON_EVENT],
                |row| {
                    let id = row.get::<_, String>(0)?;
                    let flow_id = row.get::<_, Option<String>>(1)?.ok_or_else(|| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Null,
                            "branch comparison missing flow_id".into(),
                        )
                    })?;
                    let payload_json = row.get::<_, String>(2)?;
                    let created_at = row.get::<_, i64>(3)?;
                    comparison_from_event(id, flow_id, &payload_json, created_at)
                },
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    StorageError::NotFound(format!("branch comparison {comparison_id}"))
                }
                other => StorageError::Sqlite(other),
            })
    }

    pub fn compare_observed_metric(
        &self,
        request: MetricComparisonRequest,
    ) -> Result<MetricComparisonResult, StorageError> {
        validate_metric_request(&request)?;
        let baseline = metric_value_for_step(
            self,
            &request.flow_id,
            &request.baseline_step,
            &request.metric,
        )?;
        let candidate = metric_value_for_step(
            self,
            &request.flow_id,
            &request.candidate_step,
            &request.metric,
        )?;
        let winner = metric_winner(baseline.value, candidate.value, request.direction.as_str());
        let metric = normalize_metric_name(&request.metric);
        let summary = format!(
            "Metric `{metric}` compared {} `{}` ({}) with {} `{}` ({}); {} is better; winner `{winner}`.",
            request.baseline_step,
            format_metric_value(baseline.value),
            baseline.artifact_id,
            request.candidate_step,
            format_metric_value(candidate.value),
            candidate.artifact_id,
            request.direction
        );
        let reason = format!(
            "Values were extracted from observed artifact metrics: baseline `{}`, candidate `{}`.",
            baseline.artifact_id, candidate.artifact_id
        );
        let comparison = self.record_branch_comparison(BranchComparisonRequest {
            flow_id: request.flow_id,
            baseline_step: request.baseline_step,
            candidate_step: request.candidate_step,
            summary,
            winner: Some(winner),
            reason: Some(reason),
        })?;

        Ok(MetricComparisonResult {
            comparison,
            metric,
            direction: request.direction,
            baseline_value: baseline.value,
            candidate_value: candidate.value,
            baseline_artifact_id: baseline.artifact_id,
            candidate_artifact_id: candidate.artifact_id,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
struct StepMetricValue {
    artifact_id: String,
    value: f64,
}

fn validate_request(
    store: &ProjectStore,
    request: &BranchComparisonRequest,
) -> Result<(), StorageError> {
    validate_non_empty("flow id", &request.flow_id)?;
    validate_non_empty("baseline step", &request.baseline_step)?;
    validate_non_empty("candidate step", &request.candidate_step)?;
    validate_non_empty("summary", &request.summary)?;

    if request.baseline_step == request.candidate_step {
        return Err(StorageError::InvalidInput(
            "baseline and candidate steps must be different".to_string(),
        ));
    }

    if let Some(winner) = request.winner.as_deref() {
        match winner {
            "baseline" | "candidate" | "tie" | "inconclusive" => {}
            other => {
                return Err(StorageError::InvalidInput(format!(
                    "winner must be baseline, candidate, tie, or inconclusive; got {other}"
                )));
            }
        }
    }

    let flow = store.inspect_flow(&request.flow_id)?;
    for step_id in [&request.baseline_step, &request.candidate_step] {
        if !flow
            .steps
            .iter()
            .any(|step| step.local_id == *step_id || step.id == *step_id)
        {
            return Err(StorageError::NotFound(format!(
                "step {step_id} in flow {}",
                request.flow_id
            )));
        }
    }

    Ok(())
}

fn validate_metric_request(request: &MetricComparisonRequest) -> Result<(), StorageError> {
    validate_non_empty("flow id", &request.flow_id)?;
    validate_non_empty("baseline step", &request.baseline_step)?;
    validate_non_empty("candidate step", &request.candidate_step)?;
    validate_non_empty("metric", &request.metric)?;
    match request.direction.as_str() {
        "higher" | "lower" => Ok(()),
        other => Err(StorageError::InvalidInput(format!(
            "direction must be higher or lower; got {other}"
        ))),
    }
}

fn metric_value_for_step(
    store: &ProjectStore,
    flow_id: &str,
    step_ref: &str,
    metric: &str,
) -> Result<StepMetricValue, StorageError> {
    let step_id = resolve_step_id(store, flow_id, step_ref)?;
    let mut artifacts = store
        .list_artifacts()?
        .into_iter()
        .filter(|artifact| {
            artifact.kind == "computed"
                && artifact.source_step_id.as_deref() == Some(step_id.as_str())
        })
        .collect::<Vec<_>>();
    artifacts.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.id.cmp(&left.id))
    });

    if artifacts.is_empty() {
        return Err(StorageError::NotFound(format!(
            "computed artifacts for step {step_ref} in flow {flow_id}"
        )));
    }

    let normalized_metric = normalize_metric_name(metric);
    for artifact in artifacts {
        if let Some(value) = observed_metric_value(store, &artifact, &normalized_metric)? {
            return Ok(StepMetricValue {
                artifact_id: artifact.id,
                value,
            });
        }
    }

    Err(StorageError::NotFound(format!(
        "observed metric {normalized_metric} for step {step_ref} in flow {flow_id}"
    )))
}

fn observed_metric_value(
    store: &ProjectStore,
    artifact: &ArtifactSummary,
    metric: &str,
) -> Result<Option<f64>, StorageError> {
    let marker_observation = store.observe_artifact_with_adapter(&artifact.id, "marker_report")?;
    if let Some(value) = marker_observation.metric_value(metric) {
        return Ok(Some(value));
    }

    let observation = store.observe_artifact(&artifact.id)?;
    Ok(observation.metric_value(metric))
}

fn resolve_step_id(
    store: &ProjectStore,
    flow_id: &str,
    step_ref: &str,
) -> Result<String, StorageError> {
    let flow = store.inspect_flow(flow_id)?;
    flow.steps
        .into_iter()
        .find(|step| step.local_id == step_ref || step.id == step_ref)
        .map(|step| step.id)
        .ok_or_else(|| StorageError::NotFound(format!("step {step_ref} in flow {flow_id}")))
}

fn metric_winner(baseline: f64, candidate: f64, direction: &str) -> String {
    if (baseline - candidate).abs() <= f64::EPSILON {
        return "tie".to_string();
    }
    match direction {
        "higher" if candidate > baseline => "candidate",
        "higher" => "baseline",
        "lower" if candidate < baseline => "candidate",
        "lower" => "baseline",
        _ => "inconclusive",
    }
    .to_string()
}

fn validate_non_empty(label: &str, value: &str) -> Result<(), StorageError> {
    if value.trim().is_empty() {
        Err(StorageError::InvalidInput(format!(
            "{label} must not be empty"
        )))
    } else {
        Ok(())
    }
}

fn format_metric_value(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        let formatted = format!("{value:.8}");
        formatted
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

fn comparison_payload_json(request: &BranchComparisonRequest) -> String {
    format!(
        concat!(
            "{{",
            "\"baseline_step\":\"{}\",",
            "\"candidate_step\":\"{}\",",
            "\"summary\":\"{}\",",
            "\"winner\":{},",
            "\"reason\":{}",
            "}}"
        ),
        escape_json(request.baseline_step.trim()),
        escape_json(request.candidate_step.trim()),
        escape_json(request.summary.trim()),
        optional_json_string(request.winner.as_deref().map(str::trim)),
        optional_json_string(request.reason.as_deref().map(str::trim))
    )
}

fn comparison_from_event(
    id: String,
    flow_id: String,
    payload_json: &str,
    created_at: i64,
) -> Result<BranchComparisonRecord, rusqlite::Error> {
    Ok(BranchComparisonRecord {
        id,
        flow_id,
        baseline_step: json_string_field(payload_json, "baseline_step").unwrap_or_default(),
        candidate_step: json_string_field(payload_json, "candidate_step").unwrap_or_default(),
        summary: json_string_field(payload_json, "summary").unwrap_or_default(),
        winner: json_nullable_string_field(payload_json, "winner"),
        reason: json_nullable_string_field(payload_json, "reason"),
        created_at,
    })
}

fn optional_json_string(value: Option<&str>) -> String {
    value.filter(|inner| !inner.trim().is_empty()).map_or_else(
        || "null".to_string(),
        |inner| format!("\"{}\"", escape_json(inner)),
    )
}

fn json_nullable_string_field(json: &str, field: &str) -> Option<String> {
    json_string_field(json, field)
}

fn json_string_field(json: &str, field: &str) -> Option<String> {
    let marker = format!("\"{field}\":\"");
    let start = json.find(&marker)? + marker.len();
    let rest = &json[start..];
    let end = find_json_string_end(rest)?;
    Some(unescape_json_string(&rest[..end]))
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
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        match chars.next() {
            Some('"') => output.push('"'),
            Some('\\') => output.push('\\'),
            Some('n') => output.push('\n'),
            Some('r') => output.push('\r'),
            Some('t') => output.push('\t'),
            Some(other) => output.push(other),
            None => break,
        }
    }
    output
}

fn escape_json(input: &str) -> String {
    let mut output = String::new();
    for ch in input.chars() {
        match ch {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            ch if ch.is_control() => output.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => output.push(ch),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use crate::storage::{
        ArtifactImportMode, ArtifactImportRequest, ComputedArtifactRequest, FlowDraft,
        ProjectStore, ToolSpec,
    };

    use super::{BranchComparisonRequest, MetricComparisonRequest};

    fn temp_project_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentflow-comparison-{test_name}-{}-{}",
            std::process::id(),
            crate::storage::now_unix_seconds()
        ))
    }

    fn setup_store(test_name: &str) -> (ProjectStore, PathBuf) {
        let path = temp_project_path(test_name);
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Comparison Demo")).unwrap();
        store
            .register_tool(
                ToolSpec::from_simple_yaml(
                    r#"
schema_version: agentflow.tool.v0
namespace: marker
name: marker_survival_scan
version: 0.1.0
maturity: wrapped
description: Scan marker
inputs:
  expression_table:
    type: TSV
    required: true
params:
  gene:
    type: string
    required: true
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/echo
"#,
                )
                .unwrap(),
            )
            .unwrap();
        let input_path = path.join("expression.tsv");
        fs::write(&input_path, "sample\tTP53\nA\t1.0\n").unwrap();
        let artifact_id = store
            .import_artifact(ArtifactImportRequest {
                source_path: input_path,
                artifact_type: "TSV".to_string(),
                mode: ArtifactImportMode::Reference,
            })
            .unwrap()
            .summary
            .id;
        store
            .approve_flow(
                FlowDraft::from_simple_yaml(&format!(
                    r#"
schema_version: agentflow.flow.v0
id: compare_demo
name: Compare demo
steps:
  - id: baseline
    tool: marker/marker_survival_scan
    needs: []
    inputs:
      expression_table: {artifact_id}
    params:
      gene: TP53
    outputs:
      report: baseline_report
  - id: candidate
    tool: marker/marker_survival_scan
    needs: []
    inputs:
      expression_table: {artifact_id}
    params:
      gene: EGFR
    outputs:
      report: candidate_report
"#
                ))
                .unwrap(),
                None,
            )
            .unwrap();
        (store, path)
    }

    #[test]
    fn records_lists_and_inspects_branch_comparisons() {
        let (store, path) = setup_store("record");
        let comparison = store
            .record_branch_comparison(BranchComparisonRequest {
                flow_id: "compare_demo".to_string(),
                baseline_step: "baseline".to_string(),
                candidate_step: "candidate".to_string(),
                summary: "Candidate has cleaner separation but weaker evidence.".to_string(),
                winner: Some("inconclusive".to_string()),
                reason: Some("Needs external validation.".to_string()),
            })
            .unwrap();

        assert!(comparison.id.starts_with("event_"));
        assert_eq!(comparison.winner.as_deref(), Some("inconclusive"));

        let comparisons = store.list_branch_comparisons("compare_demo").unwrap();
        assert_eq!(comparisons.len(), 1);
        assert_eq!(comparisons[0].candidate_step, "candidate");

        let inspected = store.inspect_branch_comparison(&comparison.id).unwrap();
        assert!(inspected.to_json().contains("cleaner separation"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn comparison_rejects_missing_steps_and_invalid_winner() {
        let (store, path) = setup_store("invalid");
        let error = store
            .record_branch_comparison(BranchComparisonRequest {
                flow_id: "compare_demo".to_string(),
                baseline_step: "baseline".to_string(),
                candidate_step: "missing".to_string(),
                summary: "Cannot compare missing step.".to_string(),
                winner: Some("candidate".to_string()),
                reason: None,
            })
            .unwrap_err();
        assert!(error.to_string().contains("not found: step missing"));

        let error = store
            .record_branch_comparison(BranchComparisonRequest {
                flow_id: "compare_demo".to_string(),
                baseline_step: "baseline".to_string(),
                candidate_step: "candidate".to_string(),
                summary: "Bad winner value.".to_string(),
                winner: Some("magic".to_string()),
                reason: None,
            })
            .unwrap_err();
        assert!(error.to_string().contains("winner must be"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn metric_comparison_observes_step_artifacts_and_records_winner() {
        let (store, path) = setup_store("metric");
        let baseline_path = path.join("baseline.md");
        fs::write(&baseline_path, "Gene: TP53\nscore: 0.61\n").unwrap();
        let candidate_path = path.join("candidate.md");
        fs::write(&candidate_path, "Gene: EGFR\nscore: 0.75\n").unwrap();
        let baseline_artifact = store
            .register_computed_artifact(ComputedArtifactRequest {
                source_path: baseline_path,
                artifact_type: "Markdown".to_string(),
                output_name: "report".to_string(),
                source_step_id: "step:compare_demo/baseline".to_string(),
                source_run_id: "run_baseline".to_string(),
            })
            .unwrap();
        let candidate_artifact = store
            .register_computed_artifact(ComputedArtifactRequest {
                source_path: candidate_path,
                artifact_type: "Markdown".to_string(),
                output_name: "report".to_string(),
                source_step_id: "step:compare_demo/candidate".to_string(),
                source_run_id: "run_candidate".to_string(),
            })
            .unwrap();

        let result = store
            .compare_observed_metric(MetricComparisonRequest {
                flow_id: "compare_demo".to_string(),
                baseline_step: "baseline".to_string(),
                candidate_step: "candidate".to_string(),
                metric: "Score".to_string(),
                direction: "higher".to_string(),
            })
            .unwrap();

        assert_eq!(result.metric, "score");
        assert_eq!(result.baseline_value, 0.61);
        assert_eq!(result.candidate_value, 0.75);
        assert_eq!(result.comparison.winner.as_deref(), Some("candidate"));
        assert_eq!(result.baseline_artifact_id, baseline_artifact.summary.id);
        assert_eq!(result.candidate_artifact_id, candidate_artifact.summary.id);
        assert!(result.to_json().contains("\"candidate_value\":0.75"));

        let comparisons = store.list_branch_comparisons("compare_demo").unwrap();
        assert_eq!(comparisons.len(), 1);
        assert!(comparisons[0].summary.contains("winner `candidate`"));

        let observations = store.list_observations().unwrap();
        assert_eq!(observations.len(), 2);
        assert!(observations.iter().any(|observation| {
            observation.kind == "marker_report"
                && observation.payload_json.contains("\"gene\":\"TP53\"")
        }));
        assert!(observations.iter().any(|observation| {
            observation.kind == "marker_report"
                && observation.payload_json.contains("\"gene\":\"EGFR\"")
        }));

        let _ = fs::remove_dir_all(path);
    }
}
