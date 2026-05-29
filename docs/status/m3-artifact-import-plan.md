# M3 Artifact Import Implementation Plan

Status: Implemented
Date: 2026-05-28
Scope: V0 artifact registration from existing local files

## 1. Purpose

M3 Artifact Import lets AgentFlow start from intermediate scientific data, not only from workflow roots.

This directly supports cases like:

- starting from BAM instead of FASTQ
- starting from expression tables instead of raw sequencing
- starting from an existing H5AD, count matrix, or clinical table

The goal is:

> A project can register local files as first-class artifacts with type, path, size, hash, import mode, and validation metadata.

## 2. Scope

M3 implements:

- `reference` import mode
- `copy` import mode
- imported artifact records in the `artifacts` table
- artifact listing
- artifact inspection
- versioned JSON output for artifact list and inspection
- basic validation metadata
- a small expression TSV fixture for manual CLI testing

M3 does not implement:

- flow validation
- input/output compatibility checks against tool specs
- runtime execution
- cache use
- artifact deletion
- artifact revalidation commands
- recursive directory import
- remote URI import
- Research Mode
- Omiga adapter

## 3. Import Mode Decision

Default import mode is:

```text
reference
```

Rationale:

- Bioinformatics files can be large.
- Users often start from existing BAM, H5AD, count matrix, or clinical table files.
- Silent copying can waste disk and surprise users.

Explicit copy is available:

```text
agentflow import data.tsv --type TSV --mode copy
```

Copy mode stores files under:

```text
.agentflow/artifacts/imported/<artifact_id>/<filename>
```

## 4. Hash Decision

M3 uses a built-in deterministic `fnv64` file hash.

Reason:

- The workspace guidance discourages new dependencies without explicit request.
- SHA-256 would require adding a hashing crate or using a platform command.
- M3 needs stable identity metadata, not final cache security.

This is intentionally not the final cache hash policy. A later cache/runtime milestone should revisit SHA-256 or stronger hashing with an explicit dependency decision.

## 5. CLI Surface

Implemented:

```text
agentflow import <file> --type <artifact-type> [--mode reference|copy] [--path <project>]
agentflow artifacts list [--json] [--path <project>]
agentflow artifacts inspect <artifact-id> [--json] [--path <project>]
```

## 6. Storage Behavior

Import writes:

- `artifacts`
- `events`

Validation JSON records:

- schema version
- validity flag
- import mode
- hash algorithm
- hash
- size
- source path
- stored path

## 7. Review Gate

Automatic gate:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Manual CLI gate:

```text
agentflow init --name ArtifactDemo --path <temp>
agentflow import examples/data/expression.tsv --type TSV --mode reference --path <temp>
agentflow artifacts list --json --path <temp>
agentflow artifacts inspect <artifact-id> --json --path <temp>
agentflow import examples/data/expression.tsv --type TSV --mode copy --path <temp>
```

## 8. Residual Risk

- `fnv64` is not cryptographic and should not be the final cache integrity hash.
- Artifact type is a string convention only; compatibility with tool inputs is deferred.
- Reference mode can later point to a moved or deleted file; revalidation is deferred.
- Copy mode does not deduplicate identical content.
- Directory import and remote object import are deferred.
