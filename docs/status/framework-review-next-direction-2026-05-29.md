# Framework Review and Next Direction

Date: 2026-05-29
Status: updated after update_params graph patches

## What Now Exists

AgentFlow is now a real CLI-first runtime prototype, not only a design document.

Implemented foundation:

- Independent Rust workspace: `agentflow-core`, `agentflow-cli`, `agentflow-schemas`.
- Project-local SQLite state under `.agentflow/`.
- Tool registry, artifact registry, flow validation/approval, sequential DAG execution.
- Local process executor with isolated workdirs, stdout/stderr logs, runtime input rehashing, output path containment, retry, cache explanation, and Markdown reports.
- Evidence state primitives: observations, research notes, graph patch proposals/approvals/apply, branch comparisons.
- Dynamic graph mutation first slice: approved `add_step`, constrained `add_edge`, and `update_params` with downstream invalidation.
- Targeted execution first slice: `run-step` can execute a selected draft/ready/failed step after dependency completion and rejects completed-step reruns.
- Output interpretation first slice: generic artifact observations plus a `marker_report` adapter that extracts gene/score-style evidence.
- Runtime-connected output observations: tool outputs can declare `observer: marker_report`, and runtime records observations after successful execution or cache restore.
- Deterministic port validators: tool inputs and outputs can declare `min_rows` and `required_columns`; inputs can also declare `sample_id_column` for cross-input sample identity checks, built-in input `profile` defaults, and the paired expression/survival `validator_profile` before command execution.
- Environment execution first slice: `local`, `conda`, and `micromamba` runtime backends share the same workdir/log/cache path; Conda/micromamba wrap existing environments with explicit runner/env metadata, and `env check` can verify runner/env readiness before a real run.

Current verification:

- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.

## Architecture Health

The main architectural choice still looks correct:

- Keep AgentFlow independent and CLI-first.
- Keep Omiga integration as a later adapter.
- Let AgentFlow own task state, runtime attempts, artifacts, observations, graph patches, and reports.
- Borrow Nextflow's operational discipline without rebuilding its full DSL/executor matrix.

This is meaningful because AgentFlow can already start from intermediate artifacts and preserve a causal execution record. That directly matches the product thesis: researchers often enter analysis from BAM, H5AD, tables, reports, or other partial products instead of canonical pipeline roots.

## Main Gaps

The previous biggest missing bridge has now been closed for the first adapter: output evidence generation can be part of runtime execution.

Current behavior:

- `agentflow observe` can create observations manually.
- `compare metrics` can trigger observation when comparing step outputs.
- `marker_report` proves adapter-shaped interpretation is viable.
- Tool specs can declare an output observer.
- Runtime automatically observes declared outputs after success and cache restore.

Missing behavior:

- Reports include automatic observations, but do not yet render observer payloads in a domain-specific way.
- Validator coverage is still intentionally narrow: line-oriented `min_rows`/`required_columns`, cross-input `sample_id_column`, and built-in table profile defaults.
- Targeted execution can now replay parameter changes after `update_params` invalidates the target and downstream steps.

Other important gaps remain:

- Conda/micromamba can wrap and check existing environments through explicit runner/env selection, but no environment creation or package solving exists yet.
- No planner or failure-explainer Agent layer.
- No general observer registry or adapter selection policy.
- No rollback, merge, delete, branch label, decision node, or supersede semantics.
- CLI implementation is becoming large and should eventually be split by command group.
- JSON/YAML parsing is intentionally dependency-light but increasingly hand-rolled; future schema hardening may require a more structured approach.

## Recommended Next Slice

Completed implementation slices:

> Tool-declared observers wired into runtime postflight.

> Tool-declared deterministic validators wired into runtime preflight/postflight.

> Targeted `run-step` execution with dependency gating.

> `update_params` graph patches with downstream invalidation.

Completed scope:

1. Extend tool spec parsing to allow output observer metadata, for example:

```yaml
outputs:
  report:
    type: Markdown
    observer: marker_report
```

2. Preserved backward compatibility:

- Existing tool specs without `observer` keep working.
- Existing CLI commands keep their current behavior.
- `observe --adapter marker_report` remains available for manual inspection.

3. Added runtime behavior:

- After a step succeeds and registers computed artifacts, runtime checks the output declaration.
- If an observer adapter is declared for that output, runtime records an observation automatically.
- The observation event is linked to flow, step, run, and artifact through existing storage fields.

4. Added focused tests:

- Tool spec parser accepts output observer metadata.
- A run with `observer: marker_report` automatically creates `marker_report` observations.
- Markdown report includes those observations without manual `observe`.
- Existing tools without observer metadata still pass unchanged.

5. Added validator behavior:

- Inputs can declare `required_columns`, `min_rows`, `sample_id_column`, and built-in table `profile` values.
- Tools can declare `validator_profile: paired_expression_survival_v0` to apply expression/survival input defaults without repeating them.
- Outputs can declare `required_columns` and `min_rows`.
- Input validators run before command execution.
- Output validators run before computed artifacts are published.

6. Added targeted execution behavior:

- `run-step` accepts the same step references as `retry`: unique step id, `flow.step`, `flow/step`, and `step:flow/step`.
- A selected step runs only after upstream dependencies have completed.
- Completed steps are not silently rerun.

7. Added parameter mutation behavior:

- `patch apply` supports `{"op":"update_params","step":"scan","params":{"gene":"ALK"}}`.
- Updated params are revalidated against the registered tool contract before persistence.
- The target step and downstream dependent steps are reset to `draft` for replay.
- Running steps cannot be invalidated, preventing partial parameter rewrites of active work.

## Why This Comes Before Agent Planning

Agent planning depends on trustworthy evidence. If outputs are not automatically summarized and linked to the graph, an Agent will be forced back into ad hoc file scanning and chat-memory reasoning.

This slice makes the product more scientific before it becomes more autonomous:

- input/output meaning becomes part of the tool contract
- output interpretation happens immediately after execution
- reports become causal rather than manually assembled
- future Agent hooks can consume compact observations instead of raw files

## Direction After That

The next highest-value steps are now:

1. Add better partial branch replay/status/report controls after graph changes.
2. Add branch labels and explicit decision nodes around graph patches.
3. Add richer validator profiles: file signature checks, additional schema profiles, empty-result policies, domain QC policies.
4. Add `agentflow env prepare` so Conda/micromamba environments can be created explicitly before runs.
5. Add a small planner/failure-explainer Agent layer that only proposes graph patches from existing observations, logs, tools, and research notes.

This keeps the project from drifting into fantasy autonomy. The next milestone should make AgentFlow better at seeing and recording what happened before asking it to reason more ambitiously.
