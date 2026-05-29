# M5 Runtime Execution Core Board

Bundle: M5 Runtime Execution Core
State: completed
Owner: Project Lead / Technical Director
Created: 2026-05-28

## Goal

Run an approved local flow and record execution state, workdir, logs, and outputs.

This is the first macro delivery bundle after M0-M4 foundation work.

## User-Visible Acceptance Demo

```text
agentflow init
agentflow tools register examples/tools/marker_survival_scan.tool.yaml
agentflow import examples/data/expression.tsv --type TSV
agentflow flow approve <prepared-flow.yaml>
agentflow run marker_demo
agentflow status --json
agentflow logs <attempt-id>
agentflow artifacts list --json
```

Acceptance requires:

- approved ready steps are executed by the local backend
- a downstream step can consume an upstream `step.output` artifact
- one immutable workdir is created
- `command.sh`, `inputs.json`, `params.json`, and `runtime.json` are written
- stdout and stderr are captured
- `runs` and `run_attempts` are written
- step status reaches `completed` or `failed`
- declared output files are validated
- computed outputs are registered as artifacts
- logs can be inspected from CLI

## Implementation Lanes

| Lane | State | Write Scope | Main Responsibility |
| --- | --- | --- | --- |
| Executable contract | completed | `tool_registry`, `flow_registry`, examples | Structured runtime command, required ports, output publish semantics |
| Runtime state service | completed | `crates/agentflow-core/src/runtime/*`, storage run APIs | DAG ready-step selection, state transitions, run/run_attempt records |
| Local executor | completed | `crates/agentflow-core/src/runtime/*` | workdir creation, command materialization, process execution, log capture |
| Output/artifact integration | completed | artifact service extension, runtime output validation | validate declared outputs and register computed artifacts |
| CLI runtime commands | completed | `crates/agentflow-cli/src/lib.rs` | `run`, `logs`, runtime status output |
| Integration tests | completed | unit/integration tests | end-to-end runtime demo and failure case |

## Review Lanes

| Reviewer | State | Focus |
| --- | --- | --- |
| architect | completed | state model, module boundaries, storage ownership |
| code-reviewer | completed | correctness, maintainability, scope creep |
| security-reviewer | planned | command execution, paths, filesystem safety |
| test-engineer | covered_locally | failure paths, acceptance coverage |
| verifier | completed | final demo evidence |

## Non-Goals

- cache hit/miss behavior
- retry command
- parallel execution
- Conda/Docker/Singularity
- Agent failure explanation
- Research Mode
- Omiga adapter
- Nextflow integration

## Blocking Risks

- Command execution still requires a dedicated security review before broad user-facing use.
- Current hand-rendered JSON is tolerable but may become brittle for richer runtime payloads.
- Step-output lookup uses the current V0 artifact validation metadata instead of a dedicated output mapping table; this is acceptable for M5 but should be revisited before cache/retry/report expansion.
- Runtime still uses lightweight hand-written JSON field scanning for stored V0 metadata; this is covered by regression tests but should be replaced with structured serialization before richer payloads.

## Planned Controls

- Architect review completed before runtime implementation.
- Security review before broadening local executor beyond V0 test/demo scope.
- Runtime commands execute only registered tool argv commands, not arbitrary Agent-generated shell.
- Executable tool metadata is structured at registration time.
- `status --json` keeps the existing `agentflow.status.v0.project` object and adds `counts` additively.
- Workdirs are inside `.agentflow/work/`.
- Runtime uses one workdir per attempt and does not overwrite prior attempts.
- Runtime preserves failed attempt logs.
- Final acceptance requires automated tests, clippy, and a manual CLI demo.

## Automatic Gates

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Manual Gates

Completed:

- Clean CLI demo ran successfully in `/private/tmp/agentflow-m5-demo-20260529-003`.
- Review subagent returned `PASS_WITH_RISK` after blocker fixes.
- Separate security review is deferred to the next executor hardening slice because V0 still only supports local registered argv commands and no remote/user-facing sandbox policy.

## Decision Log

- 2026-05-28: Created board after switching from micro-slice delivery to macro bundle delivery.
- 2026-05-28: Architect subagent was started for macro review but timed out and was shut down; manual architecture review is required before implementation begins.
- 2026-05-28: M5 implementation started. Architect subagent started again for read-only risk review. Test-engineer subagent could not start because the agent thread limit was reached; project lead will cover test strategy locally unless a review slot becomes available.
- 2026-05-29: Architect review returned blocking risks. M5 runtime implementation is paused until M5.0 executable contract is implemented: structured tool inputs/outputs/runtime command, output publish semantics, status JSON compatibility rule, and a blocking test for missing required tool inputs.
- 2026-05-29: M5.0 contract implemented. Tool registration now stores structured inputs, params, outputs, and local runtime argv; flow validation checks required and unknown tool ports/params.
- 2026-05-29: M5 runtime skeleton implemented. Approved flows can run local registered commands, write attempt workdirs/logs, validate non-empty outputs, publish computed artifacts, and resolve downstream `step.output` inputs.
- 2026-05-29: Automated gates passed: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`.
- 2026-05-29: Verifier found three blockers: `step.output` refs with `artifact_` producer ids, SQL `LIKE` wildcard matching for underscored output names, and runtime argv round-trip with punctuation. All three were fixed and covered by regression tests.
- 2026-05-29: Final verifier pass returned `PASS_WITH_RISK`; no blocking M5 correctness issues remain.
- 2026-05-29: Final clean CLI demo succeeded: one flow, one step, one run attempt, two imported artifacts, one computed Markdown artifact, and readable logs.
