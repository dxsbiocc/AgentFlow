# M1 Storage Review Record

Status: Automatic gate passed; manual review passed; review subagent blocked
Date: 2026-05-28
Scope: Project-local SQLite storage, migrations, and CLI project commands

## Implemented

- Project-local `.agentflow/project.db` storage.
- Project-local `.agentflow/project.toml` metadata file.
- V0 SQLite schema migration.
- Migration tracking with version, name, checksum, and timestamp.
- Migration compatibility guard for future versions and checksum drift.
- `ProjectStore::init`, `ProjectStore::open`, `ProjectStore::summary`, `ProjectStore::append_event`, `ProjectStore::applied_migrations`, and `ProjectStore::table_names`.
- CLI commands: `init`, `status`, `status --json`, and `doctor`.

## Scope Check

This slice intentionally does not implement:

- runtime execution
- task retries
- environment backends
- tool registration
- artifact import
- flow parsing
- cache resume behavior beyond table creation
- Research Mode
- network or literature retrieval
- Omiga adapter
- Docker, Singularity, Nextflow, or remote execution

## Automatic Gate Evidence

Passed:

```text
cargo fmt --all
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Test coverage now includes:

- CLI version, help, unknown-command, init/status, and doctor failure behavior
- V0 table creation
- deferred table exclusion
- migration idempotency
- future migration rejection
- migration checksum mismatch rejection
- project init idempotency
- project summary JSON schema versioning
- append-only event input validation

## Manual CLI Evidence

Passed in a temp project directory:

```text
cargo run -p agentflow-cli -- init --name ManualDemo --path /private/tmp/agentflow-v0-manual.mEW24b
cargo run -p agentflow-cli -- status --json --path /private/tmp/agentflow-v0-manual.mEW24b
cargo run -p agentflow-cli -- doctor --path /private/tmp/agentflow-v0-manual.mEW24b
```

Observed result:

- `init` created `.agentflow/project.db`.
- `status --json` returned `agentflow.status.v0`.
- `doctor` reported one applied migration.

## Review Subagent Status

Review automation remained unreliable in this session and timed out on earlier review attempts. A dedicated `code-reviewer` subagent was also started for this M1 Storage slice on 2026-05-28, but it did not return within two wait windows and was shut down. This record therefore treats the automatic gate plus direct manual review as the current release gate.

Manual review result:

> Passed with no blocking findings. The storage layer stays inside the V0 CLI-first boundary and does not introduce frontend, Omiga, Research Mode, Docker, Singularity, or Nextflow coupling.

## Residual Risk

- JSON output is hand-rendered for now. This is acceptable for the tiny M1 status payload, but later richer JSON should use structured serialization.
- SQLite uses the system library through `rusqlite` without the `bundled` feature. If portability becomes an issue, revisit this dependency choice.
- `doctor` currently checks basic project readability and migrations; deeper integrity checks should wait until flows, artifacts, and runs are implemented.
