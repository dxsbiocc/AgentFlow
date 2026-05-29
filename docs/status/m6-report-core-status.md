# M6 Report Core Status

Bundle: M6 Report Core
State: completed
Owner: implementation subagent
Created: 2026-05-29

## Goal

Add a read-only core report API that renders baseline Markdown directly from persisted flow, step, run, run_attempt, and artifact state.

## Delivered

- Added `ProjectStore::generate_report_markdown(flow_id) -> Result<String, StorageError>` in `crates/agentflow-core/src/report.rs`.
- Exported the new report module from `crates/agentflow-core/src/lib.rs`.
- Kept scope inside core only. No runtime service, registry, artifact schema, or CLI changes were made.

## Report Contents

- flow metadata and aggregate counts
- per-step status, tool, reason, inputs, params, declared outputs
- run and run_attempt details
- referenced input artifacts and produced output artifacts
- failure summary with attempt status, error class, and error message

## Tests Run

```text
cargo test -p agentflow-core report
```

Result: 2 passed, 0 failed.

## Notes

- Markdown is assembled by hand with standard-library string writes; no new dependencies were introduced.
- Artifact ID detection intentionally ignores `step.output` references such as `artifact_scan.marker_report` so report generation does not misclassify upstream step outputs as imported artifact IDs.
