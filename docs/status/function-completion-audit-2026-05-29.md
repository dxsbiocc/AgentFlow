# Function Completion Audit

Date: 2026-05-29
Scope: Current repository implementation versus the AgentFlow product/runtime plan.

## Executive Verdict

AgentFlow currently has a usable CLI-first deterministic runtime slice. It is strong enough to prove the core execution thesis, but it is not yet the full agentic research workflow product.

Completion by lens:

- CLI deterministic runtime MVP: about 85-88% complete.
- Independent AgentFlow V1 foundation: about 60-68% complete.
- Full agentic research workflow vision: about 38-46% complete.

These are engineering readiness estimates, not marketing completion percentages.

## Evidence Checked

Automated verification:

```text
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Result:

- `agentflow-cli`: 18 tests passed
- `agentflow-core`: 69 tests passed
- `agentflow-schemas`: 3 tests passed
- Clippy passed with warnings denied

Manual evidence already recorded in `docs/status/v1-usable-slice-status.md`:

- successful local run
- cache-hit run through an equivalent flow
- markdown report generation
- failed run with logs preserved
- targeted single-step execution with dependency gating
- approved parameter update patch with downstream invalidation
- failed-step retry appending another attempt

## Completion Matrix

| Area | Status | Completion | Notes |
| --- | --- | ---: | --- |
| Workspace/crate foundation | Complete for MVP | 90% | Independent Rust workspace with CLI/core/schemas and no Omiga dependency. |
| Project storage | Complete for MVP | 85% | SQLite project DB, migrations, summary, event append. |
| CLI basics | Complete for MVP | 90% | `init`, `status`, `doctor`, help/version, path handling, observations, research notes, graph patch approvals/apply, branch comparisons. |
| Tool registry | Partial | 70% | Registers executable tool specs, validates argv contract, and supports output observer plus simple port validator metadata. No tool search, promote, catalog, or progressive disclosure. |
| Artifact registry | Complete for MVP | 80% | Reference/copy import, computed artifact registration, list/inspect. No semantic artifact validation yet. |
| Static flow graph | Complete for MVP | 88% | Parse/validate/approve/inspect works for static DAGs. Approved graph patches can now materialize constrained add_step/add_edge changes and update step params in the executable DAG. |
| Runtime scheduler | Complete for MVP | 82% | Sequential ready-step DAG execution works, targeted `run-step` execution is dependency-gated, declared validators gate inputs/outputs, and declared output observers run after successful execution or cache restore. No parallelism, cancellation, or advanced resume. |
| Local executor | Usable but constrained | 70% | Workdir/logs/env isolation/output validation are implemented. Still local-process only. |
| Environment layer | Minimal | 20% | Local backend only. Conda/micromamba, Docker, Singularity, SLURM are not implemented. |
| Cache/resume | Partial | 60% | Cache key and restore work for equivalent runnable flows. Same-flow rerun is a no-op after completion; no cache list/prune UX. Flow and step cache explanation are implemented. |
| Retry and partial replay | Partial | 68% | Failed-step retry works and preserves history. Targeted `run-step` can execute draft/ready/failed steps once dependencies are complete and rejects completed-step reruns. `update_params` patches invalidate the target and downstream steps for replay. No retry policy, max attempts, or richer replay policy. |
| Logs/status | Complete for MVP | 80% | Attempt/run log reading and status counts work. Status remains coarse. |
| Report | Baseline complete | 70% | Markdown report generated from persisted evidence, including observations, graph patches, branch comparisons, and research notes. No persisted report artifact, JSON/HTML export, citation model, or redaction. |
| Validators | Partial | 58% | Tool port validation, required input/param checks, output non-empty/path containment, and declarative `min_rows`/`required_columns` checks for inputs and outputs. No richer schema profiles, sample identity checks, or pluggable validator registry yet. |
| Observer layer | Partial | 60% | Generic artifact summary observations, simple scalar metric extraction, a first `marker_report` adapter, and tool-declared postflight observation are persisted and available through CLI/runtime. No general observer registry, QC inspectors, figure/data interpretation, or adapter selection policy yet. |
| Agent planning layer | Missing | 0% | No goal-to-draft planner, failure explainer, tool recommender, or graph patch proposer. |
| Research Mode | Seed state only | 20% | Manual research notes with confidence and source context are persisted. No source adapters, literature search, citation tracking, tool-gap workflow, or anti-self-deception critique loop. |
| Dynamic graph mutation | Narrow executable slice | 60% | Graph patch propose/list/approve/reject/apply is implemented. Approved add_step branches can run, constrained add_edge can wire new steps, update_params can mutate a step and invalidate downstream replay, and branch comparisons can be recorded manually or from observed scalar metrics; no delete/merge/rollback/supersede. |
| Omiga adapter | Missing by design | 0% | Current core is independent and Omiga-ready in boundary, but no adapter/API contract package exists yet. |
| Security hardening | Partial | 55% | Absolute executable, inline shell rejection, env clearing, output escape rejection, runtime input rehashing. No sandbox/container, redaction, trust policy, or resource limits. |

## Main Mismatches

1. The project has a working runtime slice and the first state primitives for agentic reasoning, but not the agentic layer described in the product vision.
   Missing: planner, automated graph patch proposer, richer semantic patch application, hypothesis challenge, and broader tool-specific observers.

2. Cache is useful but not yet Nextflow-like resume.
   Current behavior: completed steps are skipped on the same flow; cache-hit evidence is best shown by an equivalent second flow or a newly runnable step.

3. Tool registry is executable-contract focused, not discovery focused.
   Current behavior: register/list/inspect. Missing: capability search, catalog import, candidate tools, maturity promotion.

4. Report is evidence aggregation with first automatic observations, not full scientific interpretation.
   Current behavior: flow/run/artifact provenance Markdown plus declared output observations. Missing: citations, uncertainty, negative evidence, branch rationale, and richer domain interpretation.

5. Phase 3 graph patches and comparisons are intentionally narrow.
   Current behavior: approved patches can add new executable steps, wire edges to new steps, update params with downstream invalidation, and comparisons can record baseline/candidate judgment from manual summaries or observed scalar metrics. Missing: domain-specific statistical comparison, delete/merge/rollback/supersede, and richer decision semantics.

## Recommended Next Milestones

1. Strengthen targeted execution controls:
   - clearer branch replay UX after graph patch apply
   - report/status evidence for why steps were invalidated
   - clearer status/report evidence for skipped, failed, cached, and validator-blocked steps

2. Extend graph patch materialization:
   - branch labels and decision node records
   - rollback/supersede semantics
   - richer computed comparison metrics from tool-specific observer summaries

3. Add richer validators and observer adapters:
   - schema/sample identity validators
   - tool-specific observer adapter selection policy
   - richer report rendering for observation payloads

4. Extend Research Mode beyond manual notes:
   - local/project search first
   - source/citation records
   - accessible/full-text status
   - claims with confidence and challenge prompts

5. Add one non-local environment backend:
   - micromamba/conda is the most useful next backend for scientific tools

6. Strengthen cache/report contracts:
   - SHA-256 or dependency-backed hash decision
   - cache list/prune
   - persisted report artifact/manifest
   - redaction policy

## Product Readiness Conclusion

Current AgentFlow is ready to be treated as a runnable runtime prototype and foundation.

It is not ready to be described as a full AI research workflow system yet. The next meaningful product leap is adding tool-specific observer adapters and planner prompts that consume observations/research notes/comparisons, so comparisons are grounded in domain-aware summaries rather than generic scalar extraction.
