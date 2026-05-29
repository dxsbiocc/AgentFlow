# M3 Artifact Import Review Record

Status: Automatic gate passed; manual CLI gate passed; subagent review not rerun
Date: 2026-05-28
Scope: V0 imported artifact registry only

## Implemented

- `ArtifactImportMode`.
- `ArtifactImportRequest`.
- `ProjectStore::import_artifact`.
- `ProjectStore::list_artifacts`.
- `ProjectStore::inspect_artifact`.
- `artifacts_list_json`.
- `agentflow import`.
- `agentflow artifacts list`.
- `agentflow artifacts inspect`.
- Example fixture: `examples/data/expression.tsv`.

## Scope Check

This slice intentionally does not implement:

- flow validation
- runtime execution
- tool input matching
- output artifact generation
- cache behavior
- artifact delete/move
- artifact revalidation command
- Research Mode
- Omiga adapter
- Docker, Singularity, Nextflow, or remote execution

## Automatic Gate Evidence

Passed:

```text
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Test coverage now includes:

- artifact kind parsing
- import mode parsing
- reference import
- copy import
- missing-file rejection
- artifact list JSON schema
- CLI import/list/inspect

## Manual CLI Evidence

Passed in a temp project directory:

```text
cargo run -p agentflow-cli -- init --name ArtifactDemo --path /private/tmp/agentflow-m3-artifacts.wq4t6f
cargo run -p agentflow-cli -- import examples/data/expression.tsv --type TSV --mode reference --path /private/tmp/agentflow-m3-artifacts.wq4t6f
cargo run -p agentflow-cli -- artifacts list --json --path /private/tmp/agentflow-m3-artifacts.wq4t6f
cargo run -p agentflow-cli -- artifacts inspect artifact_1779979038953282000 --json --path /private/tmp/agentflow-m3-artifacts.wq4t6f
cargo run -p agentflow-cli -- import examples/data/expression.tsv --type TSV --mode copy --path /private/tmp/agentflow-m3-artifacts.wq4t6f
```

Observed result:

- reference mode kept the original file path
- copy mode stored the file under `.agentflow/artifacts/imported/<artifact_id>/`
- `artifacts list --json` returned `agentflow.artifact_list.v0`
- `artifacts inspect --json` returned `agentflow.artifact_inspection.v0`

## Review Notes

No blocking findings from direct manual review.

The important product boundary is preserved: registering an artifact is not treated as proof that it is biologically meaningful or compatible with a tool. It only records file existence, basic metadata, and import provenance.

## Residual Risk

- `fnv64` is a pragmatic no-new-dependency hash, not a final cache/security hash.
- Reference artifacts can become stale if the external file moves.
- Artifact type strings are not yet validated against tool input ports.
- JSON is still manually rendered for small payloads; richer output should eventually move to structured serialization.
