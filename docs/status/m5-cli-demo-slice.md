# M5 CLI / demo slice status

## Scope

This slice prepares the CLI surface for the next runtime-facing commands without changing
`agentflow-core` runtime, storage, or report internals.

Commands covered:

- `agentflow report <flow-id> [--path <path>]`
- `agentflow cache explain <flow-id|step-id> [--path <path>]`
- `agentflow retry <step-id> [--path <path>]`

## Current wiring

- `report` is parsed and wired to `ProjectStore::generate_report_markdown(&flow_id)`.
- `cache explain` is parsed and wired to flow or step targets through
  `ProjectStore::cache_explain_target(&target)`.
- `retry` is parsed and wired to `ProjectStore::retry_step_ref(&step_ref)`.
- Retry targets currently support `step:<flow>/<step>`, `<flow>/<step>`,
  `<flow>.<step>`, and unique local step ids.

## CLI test plan

Implemented now:

- usage text lists the new commands
- `cache explain <flow-id>` and `cache explain <flow.step>` succeed after a runnable flow has produced cache entries
- `report <flow-id>` renders the persisted flow/run/artifact report
- `retry <step-id>` reaches the core retry path and returns deterministic errors for
  missing or non-retriable steps

Follow-up when core APIs land:

- `report --path` resolves project roots consistently outside cwd
- `retry <step-id>` rejects non-failed or non-runnable steps with a stable error message
- `retry` + `cache explain` integration covers cache invalidation or reuse semantics after retry

## Blockers

- Report output is Markdown-only; no JSON report contract exists yet.
