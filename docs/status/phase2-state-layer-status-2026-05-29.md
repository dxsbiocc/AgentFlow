# Phase 2 State Layer Status

Date: 2026-05-29
Status: completed for the CLI-first state-layer slice
Scope: Observation, Research Note, and Graph Patch state primitives usable from terminal commands.

## Delivered

- Observer core:
  - `ProjectStore::observe_artifact`
  - `ProjectStore::observe_artifact_with_adapter`
  - `ProjectStore::list_observations`
  - `ProjectStore::inspect_observation`
  - deterministic artifact summaries written to the existing `observations` table
  - text previews, line counts, size, hash, artifact kind/type/path metadata
  - `marker_report` observer adapter for gene/score-oriented Markdown/text outputs
  - runtime postflight observation for tool outputs that declare an observer adapter

- Observer CLI:
  - `agentflow observe <artifact-id> [--adapter artifact_summary|marker_report] [--json] [--path <path>]`
  - `agentflow observations list [--json] [--path <path>]`
  - `agentflow observations inspect <observation-id> [--json] [--path <path>]`

- Research note core:
  - `ProjectStore::record_research_note`
  - `ProjectStore::list_research_notes`
  - `ProjectStore::inspect_research_note`
  - confidence-limited notes: `low`, `medium`, `high`
  - event-backed storage using `research_note_recorded`

- Research note CLI:
  - `agentflow research note --problem <text> --question <text> --finding <text> ...`
  - `agentflow research list [--json] [--path <path>]`
  - `agentflow research inspect <note-id> [--json] [--path <path>]`

- Graph patch core:
  - `ProjectStore::propose_graph_patch`
  - `ProjectStore::list_graph_patches`
  - `ProjectStore::approve_graph_patch`
  - `ProjectStore::reject_graph_patch`
  - `ProjectStore::apply_graph_patch`
  - event-backed proposal and decision records
  - pending/approved/rejected status folding
  - existing-flow validation before proposal
  - approved `add_step`, constrained `add_edge`, and `update_params` materialization into the executable DAG
  - downstream step invalidation after parameter changes

- Graph patch CLI:
  - `agentflow patch propose <flow-id> --title <text> --reason <text> (--patch-json <json>|--patch-file <file>) [--json] [--path <path>]`
  - `agentflow patch list <flow-id> [--json] [--path <path>]`
  - `agentflow patch approve <patch-id> [--json] [--path <path>]`
  - `agentflow patch reject <patch-id> --reason <text> [--json] [--path <path>]`
  - `agentflow patch apply <patch-id> [--json] [--path <path>]`

- Report evidence integration:
  - Markdown reports now include observations relevant to the flow
  - Markdown reports include graph patch status/title/reasons
  - Markdown reports include research notes with confidence/source/finding
  - Markdown reports include branch comparisons between baseline and candidate steps

- Branch comparison state:
  - `ProjectStore::record_branch_comparison`
  - `ProjectStore::list_branch_comparisons`
  - `ProjectStore::inspect_branch_comparison`
  - `ProjectStore::compare_observed_metric`
  - CLI `agentflow compare steps/metrics/list/inspect`
  - comparison records capture baseline step, candidate step, summary, winner, and reason
  - metric comparisons can extract observed numeric metrics from baseline/candidate output artifacts

- Observer metric extraction:
  - text observations extract numeric metrics from common `key: value`, `key = value`, and tab-separated formats
  - metric names are normalized for comparison, for example `Adjusted P Value` becomes `adjusted_p_value`
  - metric comparisons now prefer the `marker_report` adapter before falling back to generic artifact summaries
  - tool specs can declare `outputs.<name>.observer`, making observation part of normal execution rather than only a manual command

## Product Meaning

Phase 2 does not make AgentFlow an autonomous research agent yet. It adds the missing state primitives that such an agent must use:

- observations capture what was seen in inputs and outputs
- research notes capture bounded claims, uncertainties, and source context
- graph patches capture proposed workflow changes before DAG mutation
- approvals/rejections keep the Agent from silently rewriting the workflow
- `patch apply` turns approved, validated patches into executable steps/edges
- `update_params` patches turn revised assumptions into replayable downstream work
- branch comparisons capture why one branch is favored, tied, or still inconclusive
- metric comparisons let AgentFlow ground a comparison in observed output values when tools emit simple metrics

This keeps the core principle intact: AgentFlow records and constrains reasoning before it automates it.

## Automated Evidence

```text
cargo test -p agentflow-core --lib
cargo test -p agentflow-cli --lib
```

Observed result:

- `agentflow-core`: 69 tests passed
- `agentflow-cli`: 18 tests passed

Full workspace and clippy verification should remain the release gate:

```text
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Deferred

- No Agent planner is connected yet.
- Graph patch materialization is intentionally narrow: only `add_step`, constrained `add_edge`, and `update_params` are supported.
- Applied `update_params` patches invalidate downstream step status, but do not yet create a richer invalidation report or rollback snapshot.
- Branch comparison can use observed scalar metrics, but it does not compute domain-specific statistical similarity automatically.
- Observer adapters now exist as a narrow `marker_report` slice and can be declared on outputs; broader tool-specific QC/data/figure observers are future work.
- Research Mode does not perform network, GitHub, or literature search yet.
- Research notes are not citation records and do not solve full-text access constraints.
- Graph patch JSON now has a minimal semantic parser, but no delete/merge/rollback/supersede operations.
