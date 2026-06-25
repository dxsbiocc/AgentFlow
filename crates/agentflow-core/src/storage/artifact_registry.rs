use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::domain::ArtifactKind;

use super::project_store::{
    now_unix_nanos, now_unix_seconds, project_dir, EventRecord, ProjectStore, StorageError,
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
        self.import_artifact_with_external_reference_policy(request, false)
    }

    pub fn import_artifact_allowing_external_reference(
        &self,
        request: ArtifactImportRequest,
    ) -> Result<ArtifactInspection, StorageError> {
        self.import_artifact_with_external_reference_policy(request, true)
    }

    fn import_artifact_with_external_reference_policy(
        &self,
        request: ArtifactImportRequest,
        allow_external_reference: bool,
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
        if request.mode == ArtifactImportMode::Reference && !allow_external_reference {
            validate_reference_source_is_project_internal(&canonical_source, self.root_path())?;
        }

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
            payload_json: artifact_imported_payload_json(
                &id,
                &request.artifact_type,
                request.mode,
                &stored_path,
            ),
        })?;
        self.touch_project()?;

        self.inspect_artifact(&id)
    }

    pub fn artifacts_list_json(&self, artifacts: &[ArtifactSummary]) -> String {
        artifacts_list_json_for_project(artifacts, self.root_path())
    }

    pub fn artifact_inspection_json(&self, inspection: &ArtifactInspection) -> String {
        artifact_inspection_json_for_project(inspection, self.root_path())
    }

    pub fn artifact_validation_json(&self, validation_json: &str) -> String {
        artifact_validation_json_for_project(validation_json, self.root_path())
    }

    pub fn display_artifact_path(&self, path: &Path) -> String {
        display_artifact_path_for_project(path, self.root_path())
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
            payload_json: artifact_computed_payload_json(
                &id,
                &request.output_name,
                &request.artifact_type,
                &dest_path,
            ),
        })?;

        self.inspect_artifact(&id)
    }
}

pub fn artifacts_list_json(artifacts: &[ArtifactSummary]) -> String {
    serde_json::to_string(&ArtifactsListJson {
        schema_version: agentflow_schemas::ARTIFACT_LIST_JSON_SCHEMA_V0.to_string(),
        artifacts: artifacts.iter().map(artifact_summary_json_value).collect(),
    })
    .expect("artifact list serializes to JSON")
}

fn artifacts_list_json_for_project(artifacts: &[ArtifactSummary], project_root: &Path) -> String {
    serde_json::to_string(&ArtifactsListJson {
        schema_version: agentflow_schemas::ARTIFACT_LIST_JSON_SCHEMA_V0.to_string(),
        artifacts: artifacts
            .iter()
            .map(|artifact| artifact_summary_json_value_for_project(artifact, project_root))
            .collect(),
    })
    .expect("artifact list serializes to JSON")
}

fn artifact_inspection_json(inspection: &ArtifactInspection) -> String {
    serde_json::to_string(&ArtifactInspectionJson {
        schema_version: agentflow_schemas::ARTIFACT_INSPECTION_JSON_SCHEMA_V0.to_string(),
        artifact: artifact_summary_json_value(&inspection.summary),
        validation: serde_json::from_str(&inspection.validation_json)
            .expect("stored artifact validation JSON is valid"),
    })
    .expect("artifact inspection serializes to JSON")
}

fn artifact_inspection_json_for_project(
    inspection: &ArtifactInspection,
    project_root: &Path,
) -> String {
    serde_json::to_string(&ArtifactInspectionProjectJson {
        schema_version: agentflow_schemas::ARTIFACT_INSPECTION_JSON_SCHEMA_V0.to_string(),
        artifact: artifact_summary_json_value_for_project(&inspection.summary, project_root),
        validation: sanitized_validation_json_value(&inspection.validation_json, project_root),
    })
    .expect("artifact inspection serializes to JSON")
}

fn artifact_validation_json_for_project(validation_json: &str, project_root: &Path) -> String {
    serde_json::to_string(&sanitized_validation_json_value(
        validation_json,
        project_root,
    ))
    .expect("artifact validation serializes to JSON")
}

#[derive(Debug, Serialize, Deserialize)]
struct ArtifactsListJson {
    schema_version: String,
    artifacts: Vec<ArtifactSummaryJson>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ArtifactInspectionJson {
    schema_version: String,
    artifact: ArtifactSummaryJson,
    validation: ArtifactValidationJson,
}

#[derive(Debug, Serialize)]
struct ArtifactInspectionProjectJson {
    schema_version: String,
    artifact: ArtifactSummaryJson,
    validation: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ArtifactSummaryJson {
    id: String,
    kind: String,
    #[serde(rename = "type")]
    artifact_type: String,
    path: String,
    hash: Option<String>,
    size_bytes: Option<i64>,
    source_step_id: Option<String>,
    source_run_id: Option<String>,
    created_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct ArtifactImportedPayload {
    artifact_id: String,
    #[serde(rename = "type")]
    artifact_type: String,
    mode: String,
    path: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ArtifactComputedPayload {
    artifact_id: String,
    output_name: String,
    #[serde(rename = "type")]
    artifact_type: String,
    path: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum ArtifactValidationJson {
    Imported(ImportValidationJson),
    Computed(ComputedValidationJson),
}

fn artifact_summary_json_value(artifact: &ArtifactSummary) -> ArtifactSummaryJson {
    ArtifactSummaryJson {
        id: artifact.id.clone(),
        kind: artifact.kind.clone(),
        artifact_type: artifact.artifact_type.clone(),
        path: artifact.path.display().to_string(),
        hash: artifact.hash.clone(),
        size_bytes: artifact.size_bytes,
        source_step_id: artifact.source_step_id.clone(),
        source_run_id: artifact.source_run_id.clone(),
        created_at: artifact.created_at,
    }
}

fn artifact_summary_json_value_for_project(
    artifact: &ArtifactSummary,
    project_root: &Path,
) -> ArtifactSummaryJson {
    ArtifactSummaryJson {
        id: artifact.id.clone(),
        kind: artifact.kind.clone(),
        artifact_type: artifact.artifact_type.clone(),
        path: display_artifact_path_for_project(&artifact.path, project_root),
        hash: artifact.hash.clone(),
        size_bytes: artifact.size_bytes,
        source_step_id: artifact.source_step_id.clone(),
        source_run_id: artifact.source_run_id.clone(),
        created_at: artifact.created_at,
    }
}

fn artifact_imported_payload_json(
    artifact_id: &str,
    artifact_type: &str,
    mode: ArtifactImportMode,
    path: &Path,
) -> String {
    serde_json::to_string(&ArtifactImportedPayload {
        artifact_id: artifact_id.to_string(),
        artifact_type: artifact_type.to_string(),
        mode: mode.as_str().to_string(),
        path: path.display().to_string(),
    })
    .expect("artifact imported payload serializes to JSON")
}

fn artifact_computed_payload_json(
    artifact_id: &str,
    output_name: &str,
    artifact_type: &str,
    path: &Path,
) -> String {
    serde_json::to_string(&ArtifactComputedPayload {
        artifact_id: artifact_id.to_string(),
        output_name: output_name.to_string(),
        artifact_type: artifact_type.to_string(),
        path: path.display().to_string(),
    })
    .expect("artifact computed payload serializes to JSON")
}

fn import_validation_json(
    mode: ArtifactImportMode,
    source_path: &Path,
    stored_path: &Path,
    hash: &str,
    size_bytes: u64,
) -> String {
    serde_json::to_string(&ImportValidationJson {
        schema_version: "agentflow.artifact_validation.v0".to_string(),
        valid: true,
        import_mode: mode.as_str().to_string(),
        hash_algorithm: "fnv64".to_string(),
        hash: hash.to_string(),
        size_bytes,
        source_path: source_path.display().to_string(),
        stored_path: stored_path.display().to_string(),
    })
    .expect("artifact import validation serializes to JSON")
}

fn computed_validation_json(
    output_name: &str,
    source_path: &Path,
    stored_path: &Path,
    hash: &str,
    size_bytes: u64,
) -> String {
    serde_json::to_string(&ComputedValidationJson {
        schema_version: "agentflow.artifact_validation.v0".to_string(),
        valid: true,
        artifact_origin: "computed".to_string(),
        output_name: output_name.to_string(),
        hash_algorithm: "fnv64".to_string(),
        hash: hash.to_string(),
        size_bytes,
        source_path: source_path.display().to_string(),
        stored_path: stored_path.display().to_string(),
    })
    .expect("artifact computed validation serializes to JSON")
}

fn validate_reference_source_is_project_internal(
    canonical_source: &Path,
    project_root: &Path,
) -> Result<(), StorageError> {
    let canonical_project_root = fs::canonicalize(project_root)?;
    if canonical_source.starts_with(&canonical_project_root) {
        return Ok(());
    }

    Err(StorageError::InvalidInput(format!(
        "artifact reference resolves outside the project root: {}. Use --allow-external-reference to permit an external reference explicitly, or use --mode copy to copy the file into the project.",
        external_reference_display(canonical_source)
    )))
}

fn sanitized_validation_json_value(
    validation_json: &str,
    project_root: &Path,
) -> serde_json::Value {
    let mut value: serde_json::Value =
        serde_json::from_str(validation_json).expect("stored artifact validation JSON is valid");
    if let Some(object) = value.as_object_mut() {
        for key in ["source_path", "stored_path"] {
            if let Some(path_value) = object.get_mut(key) {
                if let Some(path) = path_value.as_str() {
                    *path_value = serde_json::Value::String(display_artifact_path_for_project(
                        path,
                        project_root,
                    ));
                }
            }
        }
    }
    value
}

fn display_artifact_path_for_project(path: impl AsRef<Path>, project_root: &Path) -> String {
    let path = path.as_ref();
    let display_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let display_root =
        fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());

    if let Ok(relative_path) = display_path.strip_prefix(&display_root) {
        return if relative_path.as_os_str().is_empty() {
            ".".to_string()
        } else {
            relative_path.display().to_string()
        };
    }

    if display_path.is_absolute() {
        external_reference_display(&display_path)
    } else {
        display_path.display().to_string()
    }
}

fn external_reference_display(path: &Path) -> String {
    path.file_name().map_or_else(
        || "<external-reference>".to_string(),
        |file_name| format!("<external-reference>/{}", file_name.to_string_lossy()),
    )
}

#[derive(Debug, Serialize, Deserialize)]
struct ImportValidationJson {
    schema_version: String,
    valid: bool,
    import_mode: String,
    hash_algorithm: String,
    hash: String,
    size_bytes: u64,
    source_path: String,
    stored_path: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ComputedValidationJson {
    schema_version: String,
    valid: bool,
    artifact_origin: String,
    output_name: String,
    hash_algorithm: String,
    hash: String,
    size_bytes: u64,
    source_path: String,
    stored_path: String,
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
    fn reference_import_rejects_external_path_by_default() {
        let path = temp_project_path("reference-external-reject");
        let external_path = temp_project_path("reference-external-source");
        fs::create_dir_all(&external_path).unwrap();
        let source_path = write_input(&external_path, "external.tsv");
        let store = ProjectStore::init(&path, Some("Artifacts")).unwrap();

        let err = store
            .import_artifact(ArtifactImportRequest {
                source_path,
                artifact_type: "TSV".to_string(),
                mode: ArtifactImportMode::Reference,
            })
            .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("external reference"));
        assert!(message.contains("--allow-external-reference"));
        assert!(message.contains("--mode copy"));

        let _ = fs::remove_dir_all(path);
        let _ = fs::remove_dir_all(external_path);
    }

    #[test]
    fn reference_import_allows_external_path_with_explicit_opt_in() {
        let path = temp_project_path("reference-external-allow");
        let external_path = temp_project_path("reference-external-allowed-source");
        fs::create_dir_all(&external_path).unwrap();
        let source_path = write_input(&external_path, "external.tsv");
        let canonical_source = fs::canonicalize(&source_path).unwrap();
        let store = ProjectStore::init(&path, Some("Artifacts")).unwrap();

        let imported = store
            .import_artifact_allowing_external_reference(ArtifactImportRequest {
                source_path,
                artifact_type: "TSV".to_string(),
                mode: ArtifactImportMode::Reference,
            })
            .unwrap();

        assert_eq!(imported.summary.path, canonical_source);

        let _ = fs::remove_dir_all(path);
        let _ = fs::remove_dir_all(external_path);
    }

    #[cfg(unix)]
    #[test]
    fn reference_import_rejects_project_symlink_to_external_target() {
        use std::os::unix::fs::symlink;

        let path = temp_project_path("reference-symlink-external");
        let external_path = temp_project_path("reference-symlink-target");
        fs::create_dir_all(&external_path).unwrap();
        let source_path = write_input(&external_path, "target.tsv");
        let store = ProjectStore::init(&path, Some("Artifacts")).unwrap();
        let link_path = path.join("linked.tsv");
        symlink(&source_path, &link_path).unwrap();

        let err = store
            .import_artifact(ArtifactImportRequest {
                source_path: link_path,
                artifact_type: "TSV".to_string(),
                mode: ArtifactImportMode::Reference,
            })
            .unwrap_err();

        assert!(err.to_string().contains("external reference"));

        let _ = fs::remove_dir_all(path);
        let _ = fs::remove_dir_all(external_path);
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
    fn artifact_project_json_relativizes_internal_paths() {
        let path = temp_project_path("json-internal-relative");
        let store = ProjectStore::init(&path, Some("Artifacts")).unwrap();
        let source_path = write_input(&path, "expression.tsv");
        let imported = store
            .import_artifact(ArtifactImportRequest {
                source_path,
                artifact_type: "TSV".to_string(),
                mode: ArtifactImportMode::Reference,
            })
            .unwrap();

        let list_json = store.artifacts_list_json(&store.list_artifacts().unwrap());
        let inspect_json = store.artifact_inspection_json(&imported);

        let project_root = fs::canonicalize(&path).unwrap().display().to_string();
        assert!(list_json.contains("\"path\":\"expression.tsv\""));
        assert!(inspect_json.contains("\"path\":\"expression.tsv\""));
        assert!(inspect_json.contains("\"source_path\":\"expression.tsv\""));
        assert!(!list_json.contains(&project_root));
        assert!(!inspect_json.contains(&project_root));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn artifact_project_json_desensitizes_external_paths() {
        let path = temp_project_path("json-external-redacted");
        let external_path = temp_project_path("json-external-source");
        fs::create_dir_all(&external_path).unwrap();
        let source_path = write_input(&external_path, "external.tsv");
        let store = ProjectStore::init(&path, Some("Artifacts")).unwrap();
        let imported = store
            .import_artifact_allowing_external_reference(ArtifactImportRequest {
                source_path,
                artifact_type: "TSV".to_string(),
                mode: ArtifactImportMode::Reference,
            })
            .unwrap();

        let list_json = store.artifacts_list_json(&store.list_artifacts().unwrap());
        let inspect_json = store.artifact_inspection_json(&imported);

        let external_root = fs::canonicalize(&external_path)
            .unwrap()
            .display()
            .to_string();
        assert!(list_json.contains("\"path\":\"<external-reference>/external.tsv\""));
        assert!(inspect_json.contains("\"path\":\"<external-reference>/external.tsv\""));
        assert!(inspect_json.contains("\"source_path\":\"<external-reference>/external.tsv\""));
        assert!(!list_json.contains(&external_root));
        assert!(!inspect_json.contains(&external_root));

        let _ = fs::remove_dir_all(path);
        let _ = fs::remove_dir_all(external_path);
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

    #[test]
    fn artifact_json_outputs_are_exact_byte_and_serde_readable() {
        let summary = ArtifactSummary {
            id: "artifact_1".to_string(),
            kind: "imported".to_string(),
            artifact_type: "TSV".to_string(),
            path: PathBuf::from("/tmp/expression.tsv"),
            hash: Some("fnv64:abc123".to_string()),
            size_bytes: Some(17),
            source_step_id: None,
            source_run_id: None,
            created_at: 9,
        };
        let inspection = ArtifactInspection {
            summary: summary.clone(),
            validation_json: "{\"schema_version\":\"agentflow.artifact_validation.v0\",\"valid\":true,\"import_mode\":\"copy\",\"hash_algorithm\":\"fnv64\",\"hash\":\"fnv64:abc123\",\"size_bytes\":17,\"source_path\":\"/tmp/source.tsv\",\"stored_path\":\"/tmp/expression.tsv\"}".to_string(),
        };

        assert_eq!(
            artifacts_list_json(&[summary]),
            "{\"schema_version\":\"agentflow.artifact_list.v0\",\"artifacts\":[{\"id\":\"artifact_1\",\"kind\":\"imported\",\"type\":\"TSV\",\"path\":\"/tmp/expression.tsv\",\"hash\":\"fnv64:abc123\",\"size_bytes\":17,\"source_step_id\":null,\"source_run_id\":null,\"created_at\":9}]}"
        );
        assert_eq!(
            inspection.to_json(),
            "{\"schema_version\":\"agentflow.artifact_inspection.v0\",\"artifact\":{\"id\":\"artifact_1\",\"kind\":\"imported\",\"type\":\"TSV\",\"path\":\"/tmp/expression.tsv\",\"hash\":\"fnv64:abc123\",\"size_bytes\":17,\"source_step_id\":null,\"source_run_id\":null,\"created_at\":9},\"validation\":{\"schema_version\":\"agentflow.artifact_validation.v0\",\"valid\":true,\"import_mode\":\"copy\",\"hash_algorithm\":\"fnv64\",\"hash\":\"fnv64:abc123\",\"size_bytes\":17,\"source_path\":\"/tmp/source.tsv\",\"stored_path\":\"/tmp/expression.tsv\"}}"
        );

        let payload: ArtifactImportedPayload = serde_json::from_str(
            "{\"artifact_id\":\"artifact_1\",\"type\":\"TSV\",\"mode\":\"reference\",\"path\":\"/tmp/expression.tsv\"}",
        )
        .unwrap();
        assert_eq!(payload.artifact_id, "artifact_1");
    }

    #[test]
    fn artifact_validation_json_is_exact_byte() {
        assert_eq!(
            import_validation_json(
                ArtifactImportMode::Reference,
                Path::new("/tmp/source.tsv"),
                Path::new("/tmp/stored.tsv"),
                "fnv64:abc123",
                17,
            ),
            "{\"schema_version\":\"agentflow.artifact_validation.v0\",\"valid\":true,\"import_mode\":\"reference\",\"hash_algorithm\":\"fnv64\",\"hash\":\"fnv64:abc123\",\"size_bytes\":17,\"source_path\":\"/tmp/source.tsv\",\"stored_path\":\"/tmp/stored.tsv\"}"
        );
        assert_eq!(
            computed_validation_json(
                "report",
                Path::new("/tmp/source.md"),
                Path::new("/tmp/stored.md"),
                "fnv64:def456",
                19,
            ),
            "{\"schema_version\":\"agentflow.artifact_validation.v0\",\"valid\":true,\"artifact_origin\":\"computed\",\"output_name\":\"report\",\"hash_algorithm\":\"fnv64\",\"hash\":\"fnv64:def456\",\"size_bytes\":19,\"source_path\":\"/tmp/source.md\",\"stored_path\":\"/tmp/stored.md\"}"
        );
    }
}
