# V1 Usable Slice Status

Status: completed
Date: 2026-05-29
Scope: CLI-first AgentFlow runtime usable without Omiga integration

## Delivered

- Project lifecycle: `init`, `status`, `doctor`
- Tool registry: `tools register`, `tools list`, `tools inspect`
- Artifact registry: reference/copy import, list, inspect
- Flow lifecycle: validate, approve, inspect
- Runtime execution: approved DAG execution, targeted `run-step` execution, workdirs, stdout/stderr logs, declared input/output validators, declared output registration, declared output observers
- Cache/resume: deterministic cache key, `cache_hit` attempts, cached artifact restore for equivalent flows
- Cache explanation: flow-level and step-level `cache explain` targets
- Retry and partial replay: targeted execution through `run-step <flow.step|flow/step|step:flow/step|unique-step>` and failed-step retry through `retry <flow.step|flow/step|step:flow/step|unique-step>`
- Report: Markdown provenance report from persisted flow, step, run, attempt, and artifact state
- Phase 2/3 state primitives: artifact observations with simple metric extraction, first `marker_report` observer adapter, automatic postflight observation for declared outputs, research notes, graph patch proposal/approval/apply records, approved `add_step`/`add_edge`/`update_params` materialization, downstream invalidation after parameter changes, and branch comparisons
- Security hardening: absolute executable requirement, inline shell/interpreter command rejection, cleared child environment, output path escape rejection, runtime input re-hashing

## Automated Evidence

```text
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Result:

- `agentflow-cli`: 18 tests passed
- `agentflow-core`: 69 tests passed
- `agentflow-schemas`: 3 tests passed
- Clippy passed with warnings denied

## Manual CLI Evidence

Temporary project:

```text
/private/tmp/agentflow-v1-demo-20260529115719
```

Commands exercised:

```text
agentflow init
agentflow tools register
agentflow import
agentflow flow validate
agentflow flow approve
agentflow run
agentflow cache explain
agentflow report
agentflow logs
agentflow artifacts list
agentflow status --json
```

Observed results:

- `marker_demo` completed with `Completed steps: 1`, `Failed steps: 0`.
- First attempt produced stdout `marker scan ok`.
- Markdown report included flow metadata, step rationale, attempts, referenced inputs, produced outputs, declared output observations, and no failures.
- Equivalent second flow `marker_demo_cached` completed with attempt status `cache_hit`.
- Cache-hit logs contained `cache hit: fnv64:a00278d031e00131`.
- Failure flow `failing_demo` completed with `Failed steps: 1` and preserved stdout/stderr.
- `retry failing_demo.scan` appended a second failed attempt without deleting the first.
- Project status showed `flows: 3`, `steps: 3`, `runs: 4`, `run_attempts: 4`, `artifacts: 4`.

## Deferred

- Agent planning is intentionally not in this runtime slice.
- Phase 2 Research Mode is manual note state only; no network/literature/tool search is implemented.
- Graph patch materialization is limited to approved `add_step`, constrained `add_edge`, and `update_params` with downstream status invalidation.
- Branch comparisons can be recorded manually or from simple observed scalar metrics; `marker_report` adds a first domain-oriented adapter, but domain-specific statistical comparison is not implemented yet.
- Validator metadata includes line-oriented `min_rows`/`required_columns` plus cross-input `sample_id_column`; richer schema profiles and QC policies are future work.
- Richer cache miss diagnostics and storage lifecycle policy are future work.
- Report is Markdown-only; JSON/HTML/export artifacts are future work.
- Execution sandboxing is still local-process based; Docker/Conda/Singularity backends are future work.
- Log/report redaction policy is not implemented yet.
