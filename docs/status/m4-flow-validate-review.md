# M4 Flow Validate Review Record

Status: Automatic gate passed; manual CLI gate passed; subagent review not rerun
Date: 2026-05-28
Scope: V0 static flow validation only

## Implemented

- `FlowDraft::from_simple_yaml`.
- `ProjectStore::validate_flow`.
- `ProjectStore::approve_flow`.
- `ProjectStore::inspect_flow`.
- CLI `flow validate`.
- CLI `flow approve`.
- CLI `flow inspect`.
- Example template: `examples/flows/marker_demo.flow.yaml`.

## Scope Check

This slice intentionally does not implement:

- execution
- scheduler
- run attempts
- work directories
- cache behavior
- output artifact registration
- environment validation
- Agent graph patches
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

- flow parsing
- successful flow validation
- flow approval
- flow inspection JSON
- missing tool rejection
- missing artifact rejection
- dependency cycle rejection
- CLI validate/approve/inspect

## Manual CLI Evidence

Passed in a temp project directory:

```text
cargo run -p agentflow-cli -- init --name FlowDemo --path /private/tmp/agentflow-m4-flow.KTeENH
cargo run -p agentflow-cli -- tools register examples/tools/marker_survival_scan.tool.yaml --path /private/tmp/agentflow-m4-flow.KTeENH
cargo run -p agentflow-cli -- import examples/data/expression.tsv --type TSV --path /private/tmp/agentflow-m4-flow.KTeENH
cargo run -p agentflow-cli -- flow validate /private/tmp/agentflow-m4-flow.KTeENH/marker_demo.flow.yaml --json --path /private/tmp/agentflow-m4-flow.KTeENH
cargo run -p agentflow-cli -- flow approve /private/tmp/agentflow-m4-flow.KTeENH/marker_demo.flow.yaml --path /private/tmp/agentflow-m4-flow.KTeENH
cargo run -p agentflow-cli -- flow inspect marker_demo --json --path /private/tmp/agentflow-m4-flow.KTeENH
```

Observed result:

- validation returned `agentflow.flow_validation.v0` and `valid: true`
- approval stored one flow and one step
- inspection returned `agentflow.flow_inspection.v0`

## Review Notes

No blocking findings from direct manual review.

The main design choice is conservative: approved steps remain `draft`. Runtime readiness should be determined later by the scheduler after stronger tool input/output compatibility checks and runtime backend checks.

## Residual Risk

- Flow parsing is intentionally narrow.
- Output references are not yet checked against declared tool outputs.
- Approved flows cannot be overwritten or superseded yet.
- Runtime implementation must not assume an approved flow has executable environment readiness.
