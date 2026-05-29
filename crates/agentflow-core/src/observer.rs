use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::Path;

use rusqlite::{params, OptionalExtension};

use crate::storage::{now_unix_seconds, EventRecord, ProjectStore, StorageError};

const OBSERVATION_KIND: &str = "artifact_summary";
const MARKER_REPORT_OBSERVATION_KIND: &str = "marker_report";
const OBSERVATION_SEVERITY: &str = "info";
const OBSERVATION_SCHEMA_VERSION: &str = "agentflow.observation.v0";
const TEXT_SAMPLE_LIMIT: usize = 4096;
const PREVIEW_LINE_LIMIT: usize = 3;
const PREVIEW_CHAR_LIMIT: usize = 240;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationRecord {
    pub id: String,
    pub flow_id: Option<String>,
    pub step_id: Option<String>,
    pub artifact_id: Option<String>,
    pub kind: String,
    pub severity: String,
    pub summary: String,
    pub payload_json: String,
    pub created_at: i64,
}

impl ObservationRecord {
    pub fn metric_value(&self, metric_name: &str) -> Option<f64> {
        metric_value_from_payload(&self.payload_json, metric_name)
    }
}

impl ProjectStore {
    pub fn observe_artifact(&self, artifact_id: &str) -> Result<ObservationRecord, StorageError> {
        let (artifact, observed) = self.load_observed_artifact(artifact_id)?;
        let observation_id = observation_id_for_artifact(artifact_id);
        let summary = observation_summary(
            &artifact.summary.path,
            &artifact.summary.kind,
            &artifact.summary.artifact_type,
            &observed,
        );
        let payload_json = observation_payload_json(&artifact, &observed);

        self.record_artifact_observation(
            &artifact,
            &observation_id,
            OBSERVATION_KIND,
            OBSERVATION_SEVERITY,
            &summary,
            &payload_json,
        )
    }

    pub fn observe_artifact_with_adapter(
        &self,
        artifact_id: &str,
        adapter: &str,
    ) -> Result<ObservationRecord, StorageError> {
        match adapter.trim() {
            OBSERVATION_KIND => self.observe_artifact(artifact_id),
            MARKER_REPORT_OBSERVATION_KIND => self.observe_marker_report(artifact_id),
            "" => Err(StorageError::InvalidInput(
                "observer adapter must not be empty".to_string(),
            )),
            other => Err(StorageError::InvalidInput(format!(
                "unsupported observer adapter: {other}; supported adapters are artifact_summary and marker_report"
            ))),
        }
    }

    fn observe_marker_report(&self, artifact_id: &str) -> Result<ObservationRecord, StorageError> {
        let (artifact, observed) = self.load_observed_artifact(artifact_id)?;
        let observation_id =
            observation_id_for_artifact_kind(MARKER_REPORT_OBSERVATION_KIND, artifact_id);
        let summary = marker_report_summary(&artifact.summary.path, &observed);
        let payload_json = marker_report_payload_json(&artifact, &observed);

        self.record_artifact_observation(
            &artifact,
            &observation_id,
            MARKER_REPORT_OBSERVATION_KIND,
            OBSERVATION_SEVERITY,
            &summary,
            &payload_json,
        )
    }

    fn load_observed_artifact(
        &self,
        artifact_id: &str,
    ) -> Result<(crate::storage::ArtifactInspection, ObservedFile), StorageError> {
        if artifact_id.trim().is_empty() {
            return Err(StorageError::InvalidInput(
                "artifact id must not be empty".to_string(),
            ));
        }

        let artifact = self.inspect_artifact(artifact_id)?;
        let observed = observe_file(&artifact.summary.path)?;
        Ok((artifact, observed))
    }

    fn record_artifact_observation(
        &self,
        artifact: &crate::storage::ArtifactInspection,
        observation_id: &str,
        kind: &str,
        severity: &str,
        summary: &str,
        payload_json: &str,
    ) -> Result<ObservationRecord, StorageError> {
        let flow_id = flow_id_for_run(self, artifact.summary.source_run_id.as_deref())?;
        let created_at = now_unix_seconds();

        self.connection().execute(
            "INSERT INTO observations
             (id, flow_id, step_id, artifact_id, kind, severity, summary, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
               flow_id = excluded.flow_id,
               step_id = excluded.step_id,
               artifact_id = excluded.artifact_id,
               kind = excluded.kind,
               severity = excluded.severity,
               summary = excluded.summary,
               payload_json = excluded.payload_json,
               created_at = excluded.created_at",
            params![
                observation_id,
                flow_id.as_deref(),
                artifact.summary.source_step_id.as_deref(),
                &artifact.summary.id,
                kind,
                severity,
                summary,
                payload_json,
                created_at
            ],
        )?;

        self.append_event(EventRecord {
            flow_id: flow_id.clone(),
            step_id: artifact.summary.source_step_id.clone(),
            run_id: artifact.summary.source_run_id.clone(),
            event_type: "observation_recorded".to_string(),
            payload_json: format!(
                "{{\"observation_id\":\"{}\",\"artifact_id\":\"{}\",\"kind\":\"{}\"}}",
                escape_json(observation_id),
                escape_json(&artifact.summary.id),
                escape_json(kind)
            ),
        })?;
        self.touch_project()?;

        self.inspect_observation(observation_id)
    }

    pub fn list_observations(&self) -> Result<Vec<ObservationRecord>, StorageError> {
        let mut stmt = self.connection().prepare(
            "SELECT id, flow_id, step_id, artifact_id, kind, severity, summary, payload_json, created_at
             FROM observations
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ObservationRecord {
                id: row.get(0)?,
                flow_id: row.get(1)?,
                step_id: row.get(2)?,
                artifact_id: row.get(3)?,
                kind: row.get(4)?,
                severity: row.get(5)?,
                summary: row.get(6)?,
                payload_json: row.get(7)?,
                created_at: row.get(8)?,
            })
        })?;

        let mut observations = Vec::new();
        for row in rows {
            observations.push(row?);
        }
        Ok(observations)
    }

    pub fn inspect_observation(
        &self,
        observation_id: &str,
    ) -> Result<ObservationRecord, StorageError> {
        if observation_id.trim().is_empty() {
            return Err(StorageError::InvalidInput(
                "observation id must not be empty".to_string(),
            ));
        }

        self.connection()
            .query_row(
                "SELECT id, flow_id, step_id, artifact_id, kind, severity, summary, payload_json, created_at
                 FROM observations
                 WHERE id = ?1",
                params![observation_id],
                |row| {
                    Ok(ObservationRecord {
                        id: row.get(0)?,
                        flow_id: row.get(1)?,
                        step_id: row.get(2)?,
                        artifact_id: row.get(3)?,
                        kind: row.get(4)?,
                        severity: row.get(5)?,
                        summary: row.get(6)?,
                        payload_json: row.get(7)?,
                        created_at: row.get(8)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound(format!("observation {observation_id}")))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedFile {
    size_bytes: i64,
    hash: String,
    text: Option<TextObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TextObservation {
    line_count: i64,
    preview: String,
    sample_text: String,
    metrics: Vec<ObservedMetric>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedMetric {
    name: String,
    value: String,
    raw: String,
}

fn observe_file(path: &Path) -> Result<ObservedFile, StorageError> {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let metadata = fs::metadata(path)?;
    let mut file = fs::File::open(path)?;
    let mut hash = FNV_OFFSET;
    let mut size_bytes: i64 = 0;
    let mut newline_count: i64 = 0;
    let mut last_byte_was_newline = false;
    let mut sample = Vec::with_capacity(TEXT_SAMPLE_LIMIT);
    let mut buffer = [0_u8; 8192];

    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }

        let chunk = &buffer[..read];
        size_bytes += read as i64;
        for byte in chunk {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(FNV_PRIME);
            if *byte == b'\n' {
                newline_count += 1;
            }
        }
        if let Some(last_byte) = chunk.last() {
            last_byte_was_newline = *last_byte == b'\n';
        }
        if sample.len() < TEXT_SAMPLE_LIMIT {
            let remaining = TEXT_SAMPLE_LIMIT - sample.len();
            sample.extend_from_slice(&chunk[..remaining.min(chunk.len())]);
        }
    }

    let text = is_text_like(&sample).then(|| {
        let line_count = if size_bytes == 0 {
            0
        } else if newline_count == 0 {
            1
        } else if last_byte_was_newline {
            newline_count
        } else {
            newline_count + 1
        };
        let preview = build_preview(&sample, metadata.len() as usize > sample.len());
        let sample_text = String::from_utf8_lossy(&sample);
        let metrics = extract_text_metrics(&sample_text);
        TextObservation {
            line_count,
            preview,
            sample_text: sample_text.to_string(),
            metrics,
        }
    });

    Ok(ObservedFile {
        size_bytes,
        hash: format!("fnv64:{hash:016x}"),
        text,
    })
}

fn is_text_like(sample: &[u8]) -> bool {
    !sample.contains(&0) && std::str::from_utf8(sample).is_ok()
}

fn build_preview(sample: &[u8], sample_truncated: bool) -> String {
    let text = String::from_utf8_lossy(sample);
    let mut preview = String::new();
    let mut truncated = sample_truncated;
    let mut chars_written = 0;

    for (line_index, line) in text.lines().enumerate() {
        if line_index == PREVIEW_LINE_LIMIT {
            truncated = true;
            break;
        }
        if line_index > 0 {
            preview.push('\n');
            chars_written += 1;
        }
        for ch in line.chars() {
            if chars_written == PREVIEW_CHAR_LIMIT {
                truncated = true;
                break;
            }
            preview.push(ch);
            chars_written += 1;
        }
        if chars_written == PREVIEW_CHAR_LIMIT {
            break;
        }
    }

    if preview.is_empty() && !text.is_empty() {
        preview = text.chars().take(PREVIEW_CHAR_LIMIT).collect();
        truncated = text.chars().count() > preview.chars().count() || sample_truncated;
    }

    if truncated && !preview.ends_with("...") {
        preview.push_str("...");
    }

    preview
}

fn observation_id_for_artifact(artifact_id: &str) -> String {
    observation_id_for_artifact_kind(OBSERVATION_KIND, artifact_id)
}

fn observation_id_for_artifact_kind(kind: &str, artifact_id: &str) -> String {
    format!("observation_{kind}_{artifact_id}")
}

fn flow_id_for_run(
    store: &ProjectStore,
    run_id: Option<&str>,
) -> Result<Option<String>, StorageError> {
    let Some(run_id) = run_id else {
        return Ok(None);
    };

    store
        .connection()
        .query_row(
            "SELECT flow_id FROM runs WHERE id = ?1",
            params![run_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(StorageError::from)
}

fn observation_summary(
    path: &Path,
    artifact_kind: &str,
    artifact_type: &str,
    observed: &ObservedFile,
) -> String {
    match &observed.text {
        Some(text) => format!(
            "Artifact {} ({}/{}) is {} bytes with hash {} and {} lines.",
            path.display(),
            artifact_kind,
            artifact_type,
            observed.size_bytes,
            observed.hash,
            text.line_count
        ),
        None => format!(
            "Artifact {} ({}/{}) is {} bytes with hash {}.",
            path.display(),
            artifact_kind,
            artifact_type,
            observed.size_bytes,
            observed.hash
        ),
    }
}

fn observation_payload_json(
    artifact: &crate::storage::ArtifactInspection,
    observed: &ObservedFile,
) -> String {
    format!(
        concat!(
            "{{",
            "\"schema_version\":\"{}\",",
            "\"artifact\":{},",
            "\"text\":{}",
            "}}"
        ),
        OBSERVATION_SCHEMA_VERSION,
        observation_artifact_json(artifact, observed),
        observation_text_json(observed.text.as_ref())
    )
}

fn marker_report_payload_json(
    artifact: &crate::storage::ArtifactInspection,
    observed: &ObservedFile,
) -> String {
    format!(
        concat!(
            "{{",
            "\"schema_version\":\"{}\",",
            "\"adapter\":\"{}\",",
            "\"artifact\":{},",
            "\"domain\":{},",
            "\"text\":{}",
            "}}"
        ),
        OBSERVATION_SCHEMA_VERSION,
        MARKER_REPORT_OBSERVATION_KIND,
        observation_artifact_json(artifact, observed),
        marker_report_domain_json(observed.text.as_ref()),
        observation_text_json(observed.text.as_ref())
    )
}

fn observation_artifact_json(
    artifact: &crate::storage::ArtifactInspection,
    observed: &ObservedFile,
) -> String {
    format!(
        concat!(
            "{{",
            "\"id\":\"{}\",",
            "\"path\":\"{}\",",
            "\"kind\":\"{}\",",
            "\"type\":\"{}\",",
            "\"size_bytes\":{},",
            "\"hash\":\"{}\"",
            "}}"
        ),
        escape_json(&artifact.summary.id),
        escape_json(&artifact.summary.path.display().to_string()),
        escape_json(&artifact.summary.kind),
        escape_json(&artifact.summary.artifact_type),
        observed.size_bytes,
        escape_json(&observed.hash)
    )
}

fn marker_report_summary(path: &Path, observed: &ObservedFile) -> String {
    let Some(text) = observed.text.as_ref() else {
        return format!(
            "Marker report {} is binary or non-UTF8 and could not be interpreted.",
            path.display()
        );
    };
    let gene = extract_named_text_field(&text.sample_text, "gene")
        .or_else(|| extract_named_text_field(&text.sample_text, "marker"));
    let score = text_metric_value(text, "score");

    match (gene.as_deref(), score) {
        (Some(gene), Some(score)) => format!(
            "Marker report {} describes gene {} with score {}.",
            path.display(),
            gene,
            score
        ),
        (Some(gene), None) => format!(
            "Marker report {} describes gene {} but no score metric was found.",
            path.display(),
            gene
        ),
        (None, Some(score)) => format!(
            "Marker report {} has score {} but no gene field was found.",
            path.display(),
            score
        ),
        (None, None) => format!(
            "Marker report {} was observed, but no gene or score field was found.",
            path.display()
        ),
    }
}

fn marker_report_domain_json(text: Option<&TextObservation>) -> String {
    let Some(text) = text else {
        return "null".to_string();
    };
    let gene = extract_named_text_field(&text.sample_text, "gene")
        .or_else(|| extract_named_text_field(&text.sample_text, "marker"));
    format!(
        "{{\"gene\":{},\"score\":{}}}",
        optional_json_string(gene.as_deref()),
        text_metric_value(text, "score").unwrap_or("null")
    )
}

fn observation_text_json(text: Option<&TextObservation>) -> String {
    text.map_or_else(
        || "null".to_string(),
        |text| {
            let metrics = text
                .metrics
                .iter()
                .map(|metric| {
                    format!(
                        "{{\"name\":\"{}\",\"value\":{},\"raw\":\"{}\"}}",
                        escape_json(&metric.name),
                        metric.value,
                        escape_json(&metric.raw)
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "{{\"line_count\":{},\"preview\":\"{}\",\"metrics\":[{}]}}",
                text.line_count,
                escape_json(&text.preview),
                metrics
            )
        },
    )
}

fn extract_named_text_field(text: &str, field: &str) -> Option<String> {
    let normalized_field = normalize_metric_name(field);
    text.lines().find_map(|line| {
        let cleaned = clean_metric_line(line);
        let (name, value) = split_metric_line(cleaned)?;
        (normalize_metric_name(name) == normalized_field)
            .then(|| clean_text_field_value(value))
            .filter(|value| !value.is_empty())
    })
}

fn clean_text_field_value(value: &str) -> String {
    value
        .trim()
        .trim_matches('`')
        .trim_matches('"')
        .trim_matches('\'')
        .trim()
        .to_string()
}

fn text_metric_value<'a>(text: &'a TextObservation, metric_name: &str) -> Option<&'a str> {
    let normalized_metric = normalize_metric_name(metric_name);
    text.metrics
        .iter()
        .find(|metric| metric.name == normalized_metric)
        .map(|metric| metric.value.as_str())
}

fn optional_json_string(value: Option<&str>) -> String {
    value.map_or_else(
        || "null".to_string(),
        |inner| format!("\"{}\"", escape_json(inner)),
    )
}

pub fn normalize_metric_name(input: &str) -> String {
    let mut output = String::new();
    let mut last_was_separator = false;
    for ch in input.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            output.push(ch.to_ascii_lowercase());
            last_was_separator = false;
        } else if !output.is_empty() && !last_was_separator {
            output.push('_');
            last_was_separator = true;
        }
    }
    while output.ends_with('_') {
        output.pop();
    }
    output
}

pub fn metric_value_from_payload(payload_json: &str, metric_name: &str) -> Option<f64> {
    let normalized = normalize_metric_name(metric_name);
    if normalized.is_empty() {
        return None;
    }
    let marker = format!("\"name\":\"{}\"", escape_json(&normalized));
    let metric_start = payload_json.find(&marker)?;
    let rest = &payload_json[metric_start + marker.len()..];
    let value_marker = "\"value\":";
    let value_start = rest.find(value_marker)? + value_marker.len();
    parse_json_number_prefix(&rest[value_start..])
}

fn extract_text_metrics(text: &str) -> Vec<ObservedMetric> {
    let mut seen = BTreeSet::new();
    let mut metrics = Vec::new();

    for line in text.lines() {
        if metrics.len() >= 32 {
            break;
        }
        let cleaned = clean_metric_line(line);
        let Some((name, value_text)) = split_metric_line(cleaned) else {
            continue;
        };
        let normalized_name = normalize_metric_name(name);
        if normalized_name.is_empty() || normalized_name.len() > 64 {
            continue;
        }
        if !seen.insert(normalized_name.clone()) {
            continue;
        }
        let Some(value) = parse_first_number(value_text) else {
            continue;
        };
        metrics.push(ObservedMetric {
            name: normalized_name,
            value: format_metric_value(value),
            raw: cleaned.to_string(),
        });
    }

    metrics
}

fn clean_metric_line(line: &str) -> &str {
    line.trim()
        .trim_start_matches('#')
        .trim()
        .trim_start_matches("- ")
        .trim_start_matches("* ")
        .trim()
        .trim_matches('`')
        .trim()
}

fn split_metric_line(line: &str) -> Option<(&str, &str)> {
    [":", "=", "\t"].into_iter().find_map(|delimiter| {
        let (left, right) = line.split_once(delimiter)?;
        (!left.trim().is_empty() && !right.trim().is_empty()).then_some((left.trim(), right.trim()))
    })
}

fn parse_first_number(input: &str) -> Option<f64> {
    let mut start = None;
    let mut end = 0;
    let chars = input.char_indices().collect::<Vec<_>>();
    for (position, (index, ch)) in chars.iter().enumerate() {
        if ch.is_ascii_digit()
            || ((*ch == '-' || *ch == '+')
                && chars
                    .get(position + 1)
                    .is_some_and(|(_, next)| next.is_ascii_digit() || *next == '.'))
            || (*ch == '.'
                && chars
                    .get(position + 1)
                    .is_some_and(|(_, next)| next.is_ascii_digit()))
        {
            start = Some(*index);
            end = *index + ch.len_utf8();
            break;
        }
    }
    let start = start?;
    for (index, ch) in input[start..].char_indices().skip(1) {
        if ch.is_ascii_digit() || matches!(ch, '.' | 'e' | 'E' | '-' | '+') {
            end = start + index + ch.len_utf8();
        } else {
            break;
        }
    }
    input[start..end].parse::<f64>().ok()
}

fn parse_json_number_prefix(input: &str) -> Option<f64> {
    let trimmed = input.trim_start();
    let mut end = 0;
    for (index, ch) in trimmed.char_indices() {
        if index == 0 && matches!(ch, '-' | '+') {
            end = ch.len_utf8();
            continue;
        }
        if ch.is_ascii_digit() || matches!(ch, '.' | 'e' | 'E' | '-' | '+') {
            end = index + ch.len_utf8();
        } else {
            break;
        }
    }
    (end > 0)
        .then(|| trimmed[..end].parse::<f64>().ok())
        .flatten()
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
    use super::*;
    use crate::storage::{ArtifactImportMode, ArtifactImportRequest};
    use std::path::PathBuf;

    fn temp_project_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentflow-observer-{test_name}-{}-{}",
            std::process::id(),
            now_unix_seconds()
        ))
    }

    fn import_artifact(store: &ProjectStore, source_path: PathBuf, artifact_type: &str) -> String {
        store
            .import_artifact(ArtifactImportRequest {
                source_path,
                artifact_type: artifact_type.to_string(),
                mode: ArtifactImportMode::Reference,
            })
            .unwrap()
            .summary
            .id
    }

    #[test]
    fn observe_artifact_records_text_summary_and_supports_list_and_inspect() {
        let path = temp_project_path("text");
        let input_path = path.join("summary.tsv");
        fs::create_dir_all(&path).unwrap();
        fs::write(
            &input_path,
            "gene\tp_value\nEGFR\t0.31\nALK\t0.77\nscore: 0.82\n",
        )
        .unwrap();

        let store = ProjectStore::init(&path, Some("Observer Demo")).unwrap();
        let artifact_id = import_artifact(&store, input_path, "TSV");

        let observation = store.observe_artifact(&artifact_id).unwrap();
        assert_eq!(
            observation.id,
            format!("observation_artifact_summary_{artifact_id}")
        );
        assert_eq!(observation.kind, OBSERVATION_KIND);
        assert_eq!(observation.severity, OBSERVATION_SEVERITY);
        assert_eq!(
            observation.artifact_id.as_deref(),
            Some(artifact_id.as_str())
        );
        assert!(observation.summary.contains("(imported/TSV)"));
        assert!(observation.summary.contains("4 lines"));
        assert!(observation
            .payload_json
            .contains("\"schema_version\":\"agentflow.observation.v0\""));
        assert!(observation.payload_json.contains("\"line_count\":4"));
        assert!(observation.payload_json.contains("\"metrics\":["));
        assert!(observation.payload_json.contains("\"name\":\"score\""));
        assert_eq!(observation.metric_value("score"), Some(0.82));
        assert!(observation
            .payload_json
            .contains("\"preview\":\"gene\\tp_value\\nEGFR\\t0.31\\nALK\\t0.77...\""));

        let listed = store.list_observations().unwrap();
        assert_eq!(listed, vec![observation.clone()]);

        let inspected = store.inspect_observation(&observation.id).unwrap();
        assert_eq!(inspected, observation);

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn observe_artifact_refreshes_existing_record_for_same_artifact() {
        let path = temp_project_path("refresh");
        let input_path = path.join("notes.txt");
        fs::create_dir_all(&path).unwrap();
        fs::write(&input_path, "first line\n").unwrap();

        let store = ProjectStore::init(&path, Some("Observer Refresh")).unwrap();
        let artifact_id = import_artifact(&store, input_path.clone(), "TXT");

        let first = store.observe_artifact(&artifact_id).unwrap();
        fs::write(&input_path, "first line\nsecond line\n").unwrap();
        let second = store.observe_artifact(&artifact_id).unwrap();

        assert_eq!(second.id, first.id);
        assert!(second.payload_json.contains("\"line_count\":2"));
        assert_eq!(store.list_observations().unwrap().len(), 1);

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn observe_artifact_uses_null_text_summary_for_binary_content() {
        let path = temp_project_path("binary");
        let input_path = path.join("plot.bin");
        fs::create_dir_all(&path).unwrap();
        fs::write(&input_path, [0_u8, 159, 146, 150, 1, 2, 3]).unwrap();

        let store = ProjectStore::init(&path, Some("Observer Binary")).unwrap();
        let artifact_id = import_artifact(&store, input_path, "BIN");

        let observation = store.observe_artifact(&artifact_id).unwrap();
        assert!(observation.payload_json.contains("\"text\":null"));
        assert!(!observation.summary.contains("lines"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn marker_report_adapter_extracts_domain_fields_and_metrics() {
        let path = temp_project_path("marker-report");
        let input_path = path.join("marker.md");
        fs::create_dir_all(&path).unwrap();
        fs::write(
            &input_path,
            "# Candidate marker\nGene: EGFR\nscore: 0.75\np value: 0.031\n",
        )
        .unwrap();

        let store = ProjectStore::init(&path, Some("Marker Report")).unwrap();
        let artifact_id = import_artifact(&store, input_path, "Markdown");

        let observation = store
            .observe_artifact_with_adapter(&artifact_id, "marker_report")
            .unwrap();

        assert_eq!(
            observation.id,
            format!("observation_marker_report_{artifact_id}")
        );
        assert_eq!(observation.kind, MARKER_REPORT_OBSERVATION_KIND);
        assert!(observation.summary.contains("describes gene EGFR"));
        assert!(observation.summary.contains("score 0.75"));
        assert!(observation
            .payload_json
            .contains("\"adapter\":\"marker_report\""));
        assert!(observation
            .payload_json
            .contains("\"domain\":{\"gene\":\"EGFR\""));
        assert!(observation.payload_json.contains("\"name\":\"p_value\""));
        assert_eq!(observation.metric_value("score"), Some(0.75));

        let listed = store.list_observations().unwrap();
        assert_eq!(listed, vec![observation]);

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn metric_extraction_normalizes_common_text_formats() {
        let metrics = extract_text_metrics(
            r#"
# AUC = 0.91
- adjusted p value: 3.2e-5
score	42
"#,
        );
        assert_eq!(metrics[0].name, "auc");
        assert_eq!(metrics[0].value, "0.91");
        assert_eq!(metrics[1].name, "adjusted_p_value");
        assert_eq!(metrics[1].value, "0.000032");
        assert_eq!(metrics[2].name, "score");
        assert_eq!(metrics[2].value, "42");
    }
}
