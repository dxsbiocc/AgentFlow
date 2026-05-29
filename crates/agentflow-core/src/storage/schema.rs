pub const V0_TABLES: &[&str] = agentflow_schemas::V0_TABLES;
pub const DEFERRED_TABLES: &[&str] = agentflow_schemas::DEFERRED_TABLES;

pub const SCHEMA_MIGRATIONS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    checksum TEXT NOT NULL,
    applied_at INTEGER NOT NULL
);
"#;

pub const V0_SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    root_path TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    engine_version TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS flows (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    status TEXT NOT NULL,
    source_path TEXT,
    schema_version TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS steps (
    id TEXT PRIMARY KEY,
    flow_id TEXT NOT NULL,
    tool_ref TEXT,
    type TEXT NOT NULL,
    status TEXT NOT NULL,
    reason TEXT,
    params_json TEXT NOT NULL DEFAULT '{}',
    inputs_json TEXT NOT NULL DEFAULT '{}',
    outputs_json TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY(flow_id) REFERENCES flows(id)
);

CREATE TABLE IF NOT EXISTS edges (
    id TEXT PRIMARY KEY,
    flow_id TEXT NOT NULL,
    from_step_id TEXT NOT NULL,
    to_step_id TEXT NOT NULL,
    edge_type TEXT NOT NULL,
    FOREIGN KEY(flow_id) REFERENCES flows(id),
    FOREIGN KEY(from_step_id) REFERENCES steps(id),
    FOREIGN KEY(to_step_id) REFERENCES steps(id)
);

CREATE TABLE IF NOT EXISTS tools (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    namespace TEXT NOT NULL,
    latest_version TEXT NOT NULL,
    maturity TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS tool_versions (
    id TEXT PRIMARY KEY,
    tool_id TEXT NOT NULL,
    version TEXT NOT NULL,
    schema_version TEXT NOT NULL,
    spec_json TEXT NOT NULL,
    spec_hash TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    FOREIGN KEY(tool_id) REFERENCES tools(id)
);

CREATE TABLE IF NOT EXISTS artifacts (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    type TEXT NOT NULL,
    path TEXT NOT NULL,
    hash TEXT,
    size_bytes INTEGER,
    source_step_id TEXT,
    source_run_id TEXT,
    validation_json TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS runs (
    id TEXT PRIMARY KEY,
    flow_id TEXT NOT NULL,
    step_id TEXT NOT NULL,
    status TEXT NOT NULL,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    latest_attempt_id TEXT,
    cache_key TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY(flow_id) REFERENCES flows(id),
    FOREIGN KEY(step_id) REFERENCES steps(id)
);

CREATE TABLE IF NOT EXISTS run_attempts (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    attempt INTEGER NOT NULL,
    status TEXT NOT NULL,
    workdir TEXT,
    started_at INTEGER,
    ended_at INTEGER,
    exit_code INTEGER,
    stdout_path TEXT,
    stderr_path TEXT,
    error_class TEXT,
    error_message TEXT,
    FOREIGN KEY(run_id) REFERENCES runs(id)
);

CREATE TABLE IF NOT EXISTS cache_entries (
    cache_key TEXT PRIMARY KEY,
    tool_ref TEXT NOT NULL,
    input_hashes_json TEXT NOT NULL,
    params_hash TEXT NOT NULL,
    runtime_hash TEXT NOT NULL,
    output_artifacts_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS observations (
    id TEXT PRIMARY KEY,
    flow_id TEXT,
    step_id TEXT,
    artifact_id TEXT,
    kind TEXT NOT NULL,
    severity TEXT NOT NULL,
    summary TEXT NOT NULL,
    payload_json TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS events (
    id TEXT PRIMARY KEY,
    flow_id TEXT,
    step_id TEXT,
    run_id TEXT,
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS reports (
    id TEXT PRIMARY KEY,
    flow_id TEXT NOT NULL,
    format TEXT NOT NULL,
    path TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
"#;
