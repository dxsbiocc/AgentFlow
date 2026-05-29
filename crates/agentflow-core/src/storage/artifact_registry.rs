use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use rusqlite::{params, OptionalExtension};

use crate::domain::ArtifactKind;

use super::project_store::{
    now_unix_seconds, project_dir, EventRecord, ProjectStore, StorageError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactImportMode {
    Reference,
    Copy,
}

impl ArtifactImportMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reference => "reference",
            Self::Copy => "copy",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "reference" => Some(Self::Reference),
            "copy" => Some(Self::Copy),
            _ => None,
        }
    }
}

impl fmt::Display for ArtifactImportMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactImportRequest {
    pub source_path: PathBuf,
    pub artifact_type: String,
    pub mode: ArtifactImportMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputedArtifactRequest {
    pub source_path: PathBuf,
    pub artifact_type: String,
    pub output_name: String,
    pub source_step_id: String,
    pub source_run_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactSummary {
    pub id: String,
    pub kind: String,
    pub artifact_type: String,
    pub path: PathBuf,
    pub hash: Option<String>,
    pub size_bytes: Option<i64>,
    pub source_step_id: Option<String>,
    pub source_run_id: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactInspection {
    pub summary: ArtifactSummary,
    pub validation_json: String,
}

impl ArtifactInspection {
    pub fn to_json(&self) -> String {
        artifact_inspection_json(self)
    }
}

impl ProjectStore {
    pub fn import_artifact(
        &self,
        request: ArtifactImportRequest,
    ) -> Result<ArtifactInspection, StorageError> {
        validate_artifact_type(&request.artifact_type)?;

        let source_path = request.source_path.as_path();
        if !source_path.exists() {
            return Err(StorageError::InvalidInput(format!(
                "artifact source does not exist: {}",
                source_path.display()
            )));
        }
        if !source_path.is_file() {
            return Err(StorageError::InvalidInput(format!(
                "artifact source must be a file: {}",
                source_path.display()
            )));
        }

        let canonical_source = fs::canonicalize(source_path)?;
        let id = format!("artifact_{}", now_unix_nanos());
        let stored_path = match request.mode {
            ArtifactImportMode::Reference => canonical_source.clone(),
            ArtifactImportMode::Copy => {
                let file_name = canonical_source
                    .file_name()
                    .ok_or_else(|| {
                        StorageError::InvalidInput(format!(
                            "artifact source has no file name: {}",
                            canonical_source.display()
                        ))
                    })?
                    .to_owned();
                let dest_dir = project_dir(self.root_path())
                    .join("artifacts/imported")
                    .join(&id);
                fs::create_dir_all(&dest_dir)?;
                let dest_path = dest_dir.join(file_name);
                fs::copy(&canonical_source, &dest_path)?;
                dest_path
            }
        };

        let metadata = fs::metadata(&stored_path)?;
        let hash = hash_file_fnv64(&stored_path)?;
        let validation_json = import_validation_json(
            request.mode,
            &canonical_source,
            &stored_path,
            &hash,
            metadata.len(),
        );
        let now = now_unix_seconds();

        self.connection().execute(
            "INSERT INTO artifacts
             (id, kind, type, path, hash, size_bytes, source_step_id, source_run_id, validation_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, ?7, ?8)",
            params![
                &id,
                ArtifactKind::Imported.as_str(),
                &request.artifact_type,
                stored_path.display().to_string(),
                &hash,
                metadata.len() as i64,
                &validation_json,
                now
            ],
        )?;

        self.append_event(EventRecord {
            flow_id: None,
            step_id: None,
            run_id: None,
            event_type: "artifact_imported".to_string(),
            payload_json: format!(
                "{{\"artifact_id\":\"{}\",\"type\":\"{}\",\"mode\":\"{}\",\"path\":\"{}\"}}",
                escape_json(&id),
                escape_json(&request.artifact_type),
                request.mode,
                escape_json(&stored_path.display().to_string())
            ),
        })?;
        self.touch_project()?;

        self.inspect_artifact(&id)
    }

    pub fn list_artifacts(&self) -> Result<Vec<ArtifactSummary>, StorageError> {
        let mut stmt = self.connection().prepare(
            "SELECT id, kind, type, path, hash, size_bytes, source_step_id, source_run_id, created_at
             FROM artifacts
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ArtifactSummary {
                id: row.get(0)?,
                kind: row.get(1)?,
                artifact_type: row.get(2)?,
                path: PathBuf::from(row.get::<_, String>(3)?),
                hash: row.get(4)?,
                size_bytes: row.get(5)?,
                source_step_id: row.get(6)?,
                source_run_id: row.get(7)?,
                created_at: row.get(8)?,
            })
        })?;

        let mut artifacts = Vec::new();
        for row in rows {
            artifacts.push(row?);
        }
        Ok(artifacts)
    }

    pub fn inspect_artifact(&self, artifact_id: &str) -> Result<ArtifactInspection, StorageError> {
        if artifact_id.trim().is_empty() {
            return Err(StorageError::InvalidInput(
                "artifact id must not be empty".to_string(),
            ));
        }

        self.connection()
            .query_row(
                "SELECT id, kind, type, path, hash, size_bytes, source_step_id, source_run_id, validation_json, created_at
                 FROM artifacts
                 WHERE id = ?1",
                params![artifact_id],
                |row| {
                    Ok(ArtifactInspection {
                        summary: ArtifactSummary {
                            id: row.get(0)?,
                            kind: row.get(1)?,
                            artifact_type: row.get(2)?,
                            path: PathBuf::from(row.get::<_, String>(3)?),
                            hash: row.get(4)?,
                            size_bytes: row.get(5)?,
                            source_step_id: row.get(6)?,
                            source_run_id: row.get(7)?,
                            created_at: row.get(9)?,
                        },
                        validation_json: row.get(8)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound(format!("artifact {artifact_id}")))
    }

    pub fn register_computed_artifact(
        &self,
        request: ComputedArtifactRequest,
    ) -> Result<ArtifactInspection, StorageError> {
        validate_artifact_type(&request.artifact_type)?;
        validate_artifact_type(&request.output_name)?;

        if !request.source_path.exists() || !request.source_path.is_file() {
            return Err(StorageError::InvalidInput(format!(
                "computed artifact source must be an existing file: {}",
                request.source_path.display()
            )));
        }

        let canonical_source = fs::canonicalize(&request.source_path)?;
        let id = format!("artifact_{}", now_unix_nanos());
        let file_name = canonical_source
            .file_name()
            .ok_or_else(|| {
                StorageError::InvalidInput(format!(
                    "computed artifact source has no file name: {}",
                    canonical_source.display()
                ))
            })?
            .to_owned();
        let dest_dir = project_dir(self.root_path())
            .join("artifacts/computed")
            .join(&id);
        fs::create_dir_all(&dest_dir)?;
        let dest_path = dest_dir.join(file_name);
        fs::copy(&canonical_source, &dest_path)?;

        let metadata = fs::metadata(&dest_path)?;
        let hash = hash_file_fnv64(&dest_path)?;
        let validation_json = computed_validation_json(
            &request.output_name,
            &canonical_source,
            &dest_path,
            &hash,
            metadata.len(),
        );
        let now = now_unix_seconds();

        self.connection().execute(
            "INSERT INTO artifacts
             (id, kind, type, path, hash, size_bytes, source_step_id, source_run_id, validation_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                &id,
                ArtifactKind::Computed.as_str(),
                &request.artifact_type,
                dest_path.display().to_string(),
                &hash,
                metadata.len() as i64,
                &request.source_step_id,
                &request.source_run_id,
                &validation_json,
                now
            ],
        )?;

        self.append_event(EventRecord {
            flow_id: None,
            step_id: Some(request.source_step_id),
            run_id: Some(request.source_run_id),
            event_type: "artifact_computed".to_string(),
            payload_json: format!(
                "{{\"artifact_id\":\"{}\",\"output_name\":\"{}\",\"type\":\"{}\",\"path\":\"{}\"}}",
                escape_json(&id),
                escape_json(&request.output_name),
                escape_json(&request.artifact_type),
                escape_json(&dest_path.display().to_string())
            ),
        })?;

        self.inspect_artifact(&id)
    }
}

pub fn artifacts_list_json(artifacts: &[ArtifactSummary]) -> String {
    let items = artifacts
        .iter()
        .map(artifact_summary_json)
        .collect::<Vec<_>>()
        .join(",");

    format!(
        "{{\"schema_version\":\"{}\",\"artifacts\":[{}]}}",
        agentflow_schemas::ARTIFACT_LIST_JSON_SCHEMA_V0,
        items
    )
}

fn artifact_inspection_json(inspection: &ArtifactInspection) -> String {
    format!(
        concat!(
            "{{",
            "\"schema_version\":\"{}\",",
            "\"artifact\":{},",
            "\"validation\":{}",
            "}}"
        ),
        agentflow_schemas::ARTIFACT_INSPECTION_JSON_SCHEMA_V0,
        artifact_summary_json(&inspection.summary),
        inspection.validation_json
    )
}

fn artifact_summary_json(artifact: &ArtifactSummary) -> String {
    format!(
        concat!(
            "{{",
            "\"id\":\"{}\",",
            "\"kind\":\"{}\",",
            "\"type\":\"{}\",",
            "\"path\":\"{}\",",
            "\"hash\":{},",
            "\"size_bytes\":{},",
            "\"source_step_id\":{},",
            "\"source_run_id\":{},",
            "\"created_at\":{}",
            "}}"
        ),
        escape_json(&artifact.id),
        escape_json(&artifact.kind),
        escape_json(&artifact.artifact_type),
        escape_json(&artifact.path.display().to_string()),
        optional_json_string(artifact.hash.as_deref()),
        optional_json_i64(artifact.size_bytes),
        optional_json_string(artifact.source_step_id.as_deref()),
        optional_json_string(artifact.source_run_id.as_deref()),
        artifact.created_at
    )
}

fn import_validation_json(
    mode: ArtifactImportMode,
    source_path: &Path,
    stored_path: &Path,
    hash: &str,
    size_bytes: u64,
) -> String {
    format!(
        concat!(
            "{{",
            "\"schema_version\":\"agentflow.artifact_validation.v0\",",
            "\"valid\":true,",
            "\"import_mode\":\"{}\",",
            "\"hash_algorithm\":\"fnv64\",",
            "\"hash\":\"{}\",",
            "\"size_bytes\":{},",
            "\"source_path\":\"{}\",",
            "\"stored_path\":\"{}\"",
            "}}"
        ),
        mode,
        escape_json(hash),
        size_bytes,
        escape_json(&source_path.display().to_string()),
        escape_json(&stored_path.display().to_string())
    )
}

fn computed_validation_json(
    output_name: &str,
    source_path: &Path,
    stored_path: &Path,
    hash: &str,
    size_bytes: u64,
) -> String {
    format!(
        concat!(
            "{{",
            "\"schema_version\":\"agentflow.artifact_validation.v0\",",
            "\"valid\":true,",
            "\"artifact_origin\":\"computed\",",
            "\"output_name\":\"{}\",",
            "\"hash_algorithm\":\"fnv64\",",
            "\"hash\":\"{}\",",
            "\"size_bytes\":{},",
            "\"source_path\":\"{}\",",
            "\"stored_path\":\"{}\"",
            "}}"
        ),
        escape_json(output_name),
        escape_json(hash),
        size_bytes,
        escape_json(&source_path.display().to_string()),
        escape_json(&stored_path.display().to_string())
    )
}

fn validate_artifact_type(value: &str) -> Result<(), StorageError> {
    if value.trim().is_empty() {
        return Err(StorageError::InvalidInput(
            "artifact type must not be empty".to_string(),
        ));
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | '+'))
    {
        return Err(StorageError::InvalidInput(
            "artifact type may only contain ASCII letters, numbers, underscore, dash, dot, slash, and plus".to_string(),
        ));
    }
    Ok(())
}

fn hash_file_fnv64(path: &Path) -> Result<String, StorageError> {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut file = fs::File::open(path)?;
    let mut buffer = [0_u8; 8192];
    let mut hash = FNV_OFFSET;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        for byte in &buffer[..read] {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    }
    Ok(format!("fnv64:{hash:016x}"))
}

fn now_unix_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn optional_json_string(value: Option<&str>) -> String {
    value.map_or_else(
        || "null".to_string(),
        |inner| format!("\"{}\"", escape_json(inner)),
    )
}

fn optional_json_i64(value: Option<i64>) -> String {
    value.map_or_else(|| "null".to_string(), |inner| inner.to_string())
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
            _ => output.push(ch),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_project_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentflow-artifact-registry-{test_name}-{}-{}",
            std::process::id(),
            now_unix_seconds()
        ))
    }

    fn write_input(path: &Path, name: &str) -> PathBuf {
        let file_path = path.join(name);
        fs::write(&file_path, "sample\tvalue\nA\t1\n").unwrap();
        file_path
    }

    #[test]
    fn import_mode_parses_v0_names() {
        assert_eq!(
            ArtifactImportMode::parse("reference"),
            Some(ArtifactImportMode::Reference)
        );
        assert_eq!(
            ArtifactImportMode::parse("copy"),
            Some(ArtifactImportMode::Copy)
        );
        assert_eq!(ArtifactImportMode::parse("unknown"), None);
    }

    #[test]
    fn reference_import_registers_existing_file_without_copying() {
        let path = temp_project_path("reference");
        let store = ProjectStore::init(&path, Some("Artifacts")).unwrap();
        let source_path = write_input(&path, "expression.tsv");

        let imported = store
            .import_artifact(ArtifactImportRequest {
                source_path: source_path.clone(),
                artifact_type: "TSV".to_string(),
                mode: ArtifactImportMode::Reference,
            })
            .unwrap();

        assert_eq!(imported.summary.kind, "imported");
        assert_eq!(imported.summary.artifact_type, "TSV");
        assert_eq!(
            imported.summary.path,
            fs::canonicalize(source_path).unwrap()
        );
        assert!(imported.summary.hash.unwrap().starts_with("fnv64:"));
        assert!(imported
            .validation_json
            .contains("\"import_mode\":\"reference\""));

        let artifacts = store.list_artifacts().unwrap();
        assert_eq!(artifacts.len(), 1);

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn copy_import_places_file_under_project_artifacts() {
        let path = temp_project_path("copy");
        let store = ProjectStore::init(&path, Some("Artifacts")).unwrap();
        let source_path = write_input(&path, "survival.tsv");

        let imported = store
            .import_artifact(ArtifactImportRequest {
                source_path,
                artifact_type: "TSV".to_string(),
                mode: ArtifactImportMode::Copy,
            })
            .unwrap();

        assert!(imported.summary.path.exists());
        assert!(imported
            .summary
            .path
            .starts_with(project_dir(&path).join("artifacts/imported")));
        assert!(imported
            .validation_json
            .contains("\"import_mode\":\"copy\""));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn import_rejects_missing_file() {
        let path = temp_project_path("missing");
        let store = ProjectStore::init(&path, Some("Artifacts")).unwrap();

        let err = store
            .import_artifact(ArtifactImportRequest {
                source_path: path.join("missing.tsv"),
                artifact_type: "TSV".to_string(),
                mode: ArtifactImportMode::Reference,
            })
            .unwrap_err();
        assert!(err.to_string().contains("does not exist"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn artifact_list_json_is_versioned() {
        let path = temp_project_path("json");
        let store = ProjectStore::init(&path, Some("Artifacts")).unwrap();
        let source_path = write_input(&path, "expression.tsv");
        store
            .import_artifact(ArtifactImportRequest {
                source_path,
                artifact_type: "TSV".to_string(),
                mode: ArtifactImportMode::Reference,
            })
            .unwrap();

        let json = artifacts_list_json(&store.list_artifacts().unwrap());
        assert!(json.contains("\"schema_version\":\"agentflow.artifact_list.v0\""));
        assert!(json.contains("\"type\":\"TSV\""));

        let _ = fs::remove_dir_all(path);
    }
}
