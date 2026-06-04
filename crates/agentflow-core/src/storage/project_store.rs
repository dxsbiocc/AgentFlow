use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::migrations;

pub type EventId = String;

#[derive(Debug)]
pub enum StorageError {
    Io(io::Error),
    Sqlite(rusqlite::Error),
    NotProject(PathBuf),
    InvalidInput(String),
    IncompatibleSchema(String),
    NotFound(String),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Sqlite(error) => write!(f, "sqlite error: {error}"),
            Self::NotProject(path) => write!(
                f,
                "not an AgentFlow project: missing {}",
                project_db_path(path).display()
            ),
            Self::InvalidInput(message) => f.write_str(message),
            Self::IncompatibleSchema(message) => write!(f, "incompatible schema: {message}"),
            Self::NotFound(message) => write!(f, "not found: {message}"),
        }
    }
}

impl std::error::Error for StorageError {}

impl From<io::Error> for StorageError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for StorageError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSummary {
    pub id: String,
    pub name: String,
    pub root_path: PathBuf,
    pub engine_version: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl ProjectSummary {
    pub fn to_json(&self) -> String {
        serde_json::to_string(&ProjectSummaryJson {
            schema_version: agentflow_schemas::STATUS_JSON_SCHEMA_V0.to_string(),
            project: ProjectSummaryProjectJson {
                id: self.id.clone(),
                name: self.name.clone(),
                root_path: self.root_path.display().to_string(),
                engine_version: self.engine_version.clone(),
                created_at: self.created_at,
                updated_at: self.updated_at,
            },
        })
        .expect("project summary serializes to JSON")
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ProjectSummaryJson {
    schema_version: String,
    project: ProjectSummaryProjectJson,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProjectSummaryProjectJson {
    id: String,
    name: String,
    root_path: String,
    engine_version: String,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRecord {
    pub flow_id: Option<String>,
    pub step_id: Option<String>,
    pub run_id: Option<String>,
    pub event_type: String,
    pub payload_json: String,
}

pub struct ProjectStore {
    root_path: PathBuf,
    conn: Connection,
}

impl ProjectStore {
    pub fn init(path: impl AsRef<Path>, project_name: Option<&str>) -> Result<Self, StorageError> {
        let root_path = path.as_ref().to_path_buf();
        fs::create_dir_all(&root_path)?;
        let agentflow_dir = project_dir(&root_path);
        fs::create_dir_all(&agentflow_dir)?;

        let db_path = project_db_path(&root_path);
        let conn = open_connection(&db_path)?;
        migrations::apply_migrations(&conn)?;

        let count: i64 = conn.query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))?;
        if count == 0 {
            let now = now_unix_seconds();
            let id = format!("project_{}", now_unix_nanos());
            let name = project_name
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| default_project_name(&root_path));
            conn.execute(
                "INSERT INTO projects
                 (id, name, root_path, created_at, updated_at, engine_version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    id,
                    &name,
                    root_path.display().to_string(),
                    now,
                    now,
                    crate::ENGINE_VERSION
                ],
            )?;
            write_project_toml(&root_path, &name)?;
        }

        Ok(Self { root_path, conn })
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let root_path = path.as_ref().to_path_buf();
        let db_path = project_db_path(&root_path);
        if !db_path.exists() {
            return Err(StorageError::NotProject(root_path));
        }
        let conn = open_connection(&db_path)?;
        migrations::apply_migrations(&conn)?;
        Ok(Self { root_path, conn })
    }

    pub fn summary(&self) -> Result<ProjectSummary, StorageError> {
        self.conn
            .query_row(
                "SELECT id, name, root_path, engine_version, created_at, updated_at
             FROM projects
             ORDER BY created_at ASC
             LIMIT 1",
                [],
                |row| {
                    Ok(ProjectSummary {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        root_path: PathBuf::from(row.get::<_, String>(2)?),
                        engine_version: row.get(3)?,
                        created_at: row.get(4)?,
                        updated_at: row.get(5)?,
                    })
                },
            )
            .map_err(StorageError::from)
    }

    pub fn append_event(&self, event: EventRecord) -> Result<EventId, StorageError> {
        if event.event_type.trim().is_empty() {
            return Err(StorageError::InvalidInput(
                "event_type must not be empty".to_string(),
            ));
        }
        let id = format!("event_{}", now_unix_nanos());
        self.conn.execute(
            "INSERT INTO events
             (id, flow_id, step_id, run_id, event_type, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                event.flow_id,
                event.step_id,
                event.run_id,
                event.event_type,
                event.payload_json,
                now_unix_seconds()
            ],
        )?;
        Ok(id)
    }

    pub fn applied_migrations(&self) -> Result<Vec<migrations::MigrationRecord>, StorageError> {
        migrations::applied_migrations(&self.conn)
    }

    pub fn table_names(&self) -> Result<Vec<String>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT name FROM sqlite_master
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
             ORDER BY name ASC",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;

        let mut names = Vec::new();
        for row in rows {
            names.push(row?);
        }
        Ok(names)
    }

    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    pub(crate) fn connection(&self) -> &Connection {
        &self.conn
    }

    pub(crate) fn touch_project(&self) -> Result<(), StorageError> {
        self.conn.execute(
            "UPDATE projects SET updated_at = ?1",
            params![now_unix_seconds()],
        )?;
        Ok(())
    }
}

pub fn project_dir(root_path: &Path) -> PathBuf {
    root_path.join(".agentflow")
}

pub fn project_db_path(root_path: &Path) -> PathBuf {
    project_dir(root_path).join("project.db")
}

pub fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn now_unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn open_connection(db_path: &Path) -> Result<Connection, StorageError> {
    let conn = Connection::open(db_path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(conn)
}

fn default_project_name(root_path: &Path) -> String {
    root_path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("AgentFlow Project")
        .to_string()
}

fn write_project_toml(root_path: &Path, name: &str) -> Result<(), StorageError> {
    let path = project_dir(root_path).join("project.toml");
    if path.exists() {
        return Ok(());
    }
    fs::write(
        path,
        format!(
            "name = \"{}\"\nengine_version = \"{}\"\n",
            escape_toml(name),
            crate::ENGINE_VERSION
        ),
    )?;
    Ok(())
}

fn escape_toml(input: &str) -> String {
    input.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_project_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentflow-{test_name}-{}-{}",
            std::process::id(),
            now_unix_nanos()
        ))
    }

    #[test]
    fn init_creates_project_db_and_summary() {
        let path = temp_project_path("init");
        let store = ProjectStore::init(&path, Some("Demo")).unwrap();
        assert!(project_db_path(&path).exists());

        let summary = store.summary().unwrap();
        assert_eq!(summary.name, "Demo");
        assert_eq!(summary.engine_version, crate::ENGINE_VERSION);

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn init_is_idempotent() {
        let path = temp_project_path("idempotent");
        let first = ProjectStore::init(&path, Some("Demo")).unwrap();
        let first_summary = first.summary().unwrap();
        drop(first);

        let second = ProjectStore::init(&path, Some("Other")).unwrap();
        let second_summary = second.summary().unwrap();
        assert_eq!(first_summary.id, second_summary.id);
        assert_eq!(second_summary.name, "Demo");

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn opening_non_project_fails() {
        let path = temp_project_path("missing");
        match ProjectStore::open(&path) {
            Ok(_) => panic!("opening a non-project path should fail"),
            Err(error) => assert!(matches!(error, StorageError::NotProject(_))),
        }
    }

    #[test]
    fn migrations_create_v0_tables_only() {
        let path = temp_project_path("tables");
        let store = ProjectStore::init(&path, Some("Demo")).unwrap();
        let tables = store.table_names().unwrap();

        for table in agentflow_schemas::V0_TABLES {
            assert!(tables.contains(&table.to_string()), "missing table {table}");
        }
        for table in agentflow_schemas::DEFERRED_TABLES {
            assert!(
                !tables.contains(&table.to_string()),
                "deferred table {table} should not exist"
            );
        }

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn migrations_are_idempotent() {
        let path = temp_project_path("migrations");
        let store = ProjectStore::init(&path, Some("Demo")).unwrap();
        let before = store.applied_migrations().unwrap();
        drop(store);

        let store = ProjectStore::init(&path, Some("Demo")).unwrap();
        let after = store.applied_migrations().unwrap();
        assert_eq!(before, after);

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn append_event_requires_event_type() {
        let path = temp_project_path("event");
        let store = ProjectStore::init(&path, Some("Demo")).unwrap();
        let error = store
            .append_event(EventRecord {
                flow_id: None,
                step_id: None,
                run_id: None,
                event_type: " ".to_string(),
                payload_json: "{}".to_string(),
            })
            .unwrap_err();

        assert!(matches!(error, StorageError::InvalidInput(_)));
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn summary_json_is_versioned() {
        let summary = ProjectSummary {
            id: "project_1".to_string(),
            name: "Demo".to_string(),
            root_path: PathBuf::from("/tmp/demo"),
            engine_version: "0.1.0".to_string(),
            created_at: 1,
            updated_at: 2,
        };

        let json = summary.to_json();
        assert!(json.contains("\"schema_version\":\"agentflow.status.v0\""));
        assert!(json.contains("\"name\":\"Demo\""));
    }

    #[test]
    fn summary_json_is_exact_byte_and_serde_readable() {
        let summary = ProjectSummary {
            id: "project_1".to_string(),
            name: "Demo \"A\"".to_string(),
            root_path: PathBuf::from("/tmp/demo path"),
            engine_version: "0.1.0".to_string(),
            created_at: 1,
            updated_at: 2,
        };

        let json = summary.to_json();
        assert_eq!(
            json,
            "{\"schema_version\":\"agentflow.status.v0\",\"project\":{\"id\":\"project_1\",\"name\":\"Demo \\\"A\\\"\",\"root_path\":\"/tmp/demo path\",\"engine_version\":\"0.1.0\",\"created_at\":1,\"updated_at\":2}}"
        );

        let payload: ProjectSummaryJson = serde_json::from_str(&json).unwrap();
        assert_eq!(payload.project.name, "Demo \"A\"");
    }
}
