# M1 Storage Implementation Plan

Status: Implemented
Date: 2026-05-28
Scope: V0 project storage, migrations, and project init/open

## 1. Purpose

M1 Storage turns the approved V0 contracts into a minimal durable project store.

The goal is not to build the full runtime yet. The goal is:

> `agentflow init` can create a project-local `.agentflow/` directory and SQLite database with the V0 schema, and core services can open the project and append events through one controlled storage boundary.

## 2. Scope

M1 Storage should implement:

- `.agentflow/` directory creation
- `.agentflow/project.db`
- `.agentflow/project.toml` or equivalent minimal project metadata
- V0 SQLite migrations
- schema migration tracking
- project init/open repository functions
- append-only event insert
- basic project status read
- migration tests

M1 Storage should not implement:

- tool registration
- artifact import
- flow parsing
- runtime execution
- cache behavior beyond table creation
- retry behavior
- report generation
- Research Mode
- catalog/capability search
- Omiga adapter
- Docker/Nextflow/Singularity

## 3. Dependency Decision

Implementing actual SQLite storage requires a Rust SQLite dependency.

Recommended choice:

> `rusqlite`

Rationale:

- Embedded/local-first SQLite use case is simple.
- V0 does not need async database access.
- V0 should use a single writer path, which fits synchronous SQLite well.
- Less abstraction than `sqlx`, lower setup complexity for a local CLI.
- Easier to keep migrations explicit and inspectable.

Rejected for V0:

- `sqlx`: useful for async service architectures and compile-time query checking, but heavier for this local CLI phase.
- calling `sqlite3` command: avoids a Rust dependency, but makes storage less portable and weaker as a core library boundary.
- custom file/JSON store: faster to start, but immediately diverges from the product requirement that SQLite is the project state source.

Because the workspace guidance says no new dependencies without explicit request, M1 implementation waited for approval to add `rusqlite`. The user then asked to continue, and this was treated as approval for the storage dependency decision in this plan.

## 4. Module Ownership

Recommended modules:

```text
crates/agentflow-core/src/storage/
  mod.rs
  migrations.rs
  project_store.rs
  schema.rs
```

Ownership rules:

- CLI must not write SQLite directly.
- Storage module owns migrations and connection setup.
- Runtime later writes `runs`, `run_attempts`, `events`, and `cache_entries` through core services.
- Tooling later writes `tools` and `tool_versions` through ToolRegistryService.
- Report later writes `reports` through ReportService.

## 5. V0 Migration Tables

M1 should create exactly the V0 table contract:

```text
schema_migrations
projects
flows
steps
edges
tools
tool_versions
artifacts
runs
run_attempts
cache_entries
observations
events
reports
```

M1 must not create deferred tables:

```text
tool_catalog_entries
tool_capabilities
hypotheses
research_notes
research_sources
research_queries
research_hits
research_documents
citations
evidence_claims
approvals
```

## 6. Minimal Core API

M1 should expose a small API from `agentflow-core`.

```text
ProjectStore::init(path, project_name) -> ProjectSummary
ProjectStore::open(path) -> ProjectStore
ProjectStore::summary() -> ProjectSummary
ProjectStore::append_event(event) -> EventId
ProjectStore::applied_migrations() -> Vec<MigrationRecord>
```

Suggested data structs:

```text
ProjectSummary
  id
  name
  root_path
  engine_version
  created_at
  updated_at

EventRecord
  flow_id?
  step_id?
  run_id?
  event_type
  payload_json
```

## 7. CLI Surface

M1 may add:

```text
agentflow init [--name <name>] [--path <path>]
agentflow status
agentflow status --json
agentflow doctor
```

Behavior:

- `init` creates `.agentflow/` and initializes database.
- `status` shows project metadata and basic counts.
- `status --json` includes schema version and engine version.
- `doctor` checks whether `.agentflow/project.db` exists and migrations are applied.

Commands that remain unimplemented:

```text
agentflow run
agentflow tools register
agentflow import
agentflow flow validate
```

These should still fail clearly until their milestones.

## 8. Migration Policy

Migration rules:

1. Migrations are forward-only.
2. Each migration has `version`, `name`, `checksum`, and `applied_at`.
3. Running migrations twice is safe.
4. Unknown future schema version should fail open with a clear error, not mutate the DB.
5. WAL mode can be enabled during DB initialization.
6. Large logs/artifacts must never be stored as blobs.

## 9. Tests Required

Unit tests:

- V0 table contract matches migration table list.
- Deferred tables are not created.
- Migration SQL is idempotent.
- Event append requires valid event type.

Integration tests:

- init creates `.agentflow/project.db`.
- open returns project summary.
- init twice is safe or returns a clear already-initialized error.
- doctor detects missing project DB.
- status JSON is parseable and versioned.

Failure tests:

- opening a non-project path fails clearly.
- corrupt/missing schema migration table fails clearly.

## 10. Review Gate

Automatic gate:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
agentflow init in a temp directory
agentflow status --json in that directory
agentflow doctor in that directory
```

Review focus:

- No deferred tables created.
- CLI does not write SQLite directly.
- Storage module is the only DB writer.
- Migration tests prove idempotency.
- No Omiga, Research Mode, Docker, Nextflow, or runtime execution introduced.
- JSON output is stable and versioned.

## 11. Dependency Approval Record

M1 implementation needs approval for one new dependency:

```text
rusqlite
```

Recommended optional feature:

```text
bundled
```

Tradeoff:

- `rusqlite` without `bundled`: uses system SQLite, lighter build.
- `rusqlite` with `bundled`: more reproducible local builds, heavier compile.

Recommendation for V0:

> Use `rusqlite` without `bundled` first, unless local build portability becomes a problem.

Implementation update:

> `rusqlite` was added without the `bundled` feature for M1 Storage.
