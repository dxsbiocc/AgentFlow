# Launch Readiness for Technical Preview

Date: 2026-05-29
Status: scope-ready for a controlled technical preview after the V1 acceptance gate passes
Scope: documentation and operator guidance for the current CLI-first AgentFlow runtime slice

## Release Positioning

AgentFlow is ready to ship as a technical preview for local, operator-driven workflow demos once the workspace verification gates are green.

It should not be positioned as:

- a packaged end-user product
- a hardened production workflow runner
- a full autonomous scientific research agent

The correct message is narrower:

- AgentFlow already proves the core runtime thesis.
- Users can register tools, import artifacts, validate and approve flows, run approved steps locally, inspect status/logs/artifacts, and generate Markdown reports.
- The reasoning and execution-control primitives now exist, but the autonomous agent layer and hardened runtime environments do not.

## Quick Start

Current repo entrypoint:

```bash
cargo run -q -p agentflow-cli -- help
```

Demo bootstrap:

```bash
export AF_DEMO=/private/tmp/agentflow-tech-preview-demo
rm -rf "$AF_DEMO"

cargo run -q -p agentflow-cli -- init --name TechPreviewDemo --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- tools register examples/tools/marker_survival_scan.tool.yaml --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- import examples/data/expression.tsv --type TSV --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- import examples/data/survival.tsv --type TSV --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- artifacts list --json --path "$AF_DEMO"
```

Prepare the sample flow:

1. Copy `examples/flows/marker_demo.flow.yaml` into the demo directory.
2. Replace the two placeholder artifact IDs with the imported IDs from `artifacts list --json`.
3. Validate and approve the prepared flow.

```bash
cp examples/flows/marker_demo.flow.yaml "$AF_DEMO/marker_demo.flow.yaml"

cargo run -q -p agentflow-cli -- flow validate "$AF_DEMO/marker_demo.flow.yaml" --json --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- flow approve "$AF_DEMO/marker_demo.flow.yaml" --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- run marker_demo --path "$AF_DEMO"
```

Post-run inspection:

```bash
cargo run -q -p agentflow-cli -- status --json --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- report marker_demo --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- artifacts list --json --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- observations list --json --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- cache explain marker_demo.scan --path "$AF_DEMO"
```

## Supported Capabilities

Current technical preview supports:

- Project creation and health checks through `init`, `status`, and `doctor`
- Registered tool contracts with declared inputs, params, outputs, validators, and local argv runtime metadata
- Artifact import by reference or copy, plus artifact list/inspect
- Static flow validation, approval, and inspection
- Sequential local execution of approved DAGs
- Dependency-gated targeted step execution through `run-step`
- Failed-step retry through `retry`
- Run and attempt inventory through `runs list`, `runs inspect`, and readable `logs`
- Attempt workdirs, command materialization, and stdout/stderr capture
- Declared output publication as artifacts
- Input/output validation with `required_columns`, `min_rows`, cross-input `sample_id_column`, built-in input `profile`, and paired expression/survival `validator_profile`
- Cache-hit restore for equivalent executable flows plus `cache explain`
- Cache inventory and explicit cache pruning through `cache list` and `cache prune`
- Local runtime timeout control through `runtime.timeout_seconds`
- Markdown report generation from persisted runtime state
- Observation, research-note, graph-patch, and branch-comparison state primitives
- Output observation adapters, including automatic `marker_report` observation when declared in a tool spec

## Explicitly Not Supported

This technical preview does not support:

- autonomous planner or failure-explainer agents
- automatic tool discovery or recommendation workflows
- dynamic graph authoring from natural-language goals
- remote or isolated runtime backends such as Conda, Docker, Singularity, or SLURM
- parallel execution scheduling
- sandboxing, container hardening, resource quotas, or redaction policy
- rich validator families beyond line-oriented table checks
- delete/merge/rollback/supersede graph patch semantics
- first-class branch labels or decision-node UX
- persisted report artifacts or JSON/HTML exports
- citation-aware research workflows, literature search, or networked source retrieval
- automated cache eviction policies or artifact garbage collection

## Verification Commands

Primary launch gate:

```bash
scripts/acceptance-v1.sh
```

The script runs the Rust quality gates and a marker-demo CLI smoke test. The individual checks are:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -q -p agentflow-cli -- help
```

Recommended demo checks:

```bash
cargo run -q -p agentflow-cli -- flow validate "$AF_DEMO/marker_demo.flow.yaml" --json --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- run marker_demo --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- status --json --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- runs list --flow marker_demo --json --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- report marker_demo --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- cache explain marker_demo.scan --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- cache list --json --path "$AF_DEMO"
```

## Example Technical Preview Flow

Operator-facing sample flow:

1. Register `examples/tools/marker_survival_scan.tool.yaml`.
2. Import `examples/data/expression.tsv` and `examples/data/survival.tsv`.
3. Prepare `marker_demo.flow.yaml` with imported artifact IDs.
4. Validate and approve the flow.
5. Run `marker_demo`.
6. Confirm:
   - flow run completes
   - `runs list` and `runs inspect` expose run/attempt state
   - output artifact is registered
   - `report marker_demo` renders Markdown provenance
   - `observations list` contains the automatic `marker_report` observation
   - `cache explain marker_demo.scan` returns a stable explanation for the demo step
   - `cache list --json` exposes the cached step for operator inspection

This is the intended preview narrative because it exercises tool registration, artifact import, validation, approval, execution, observation, report generation, and cache explanation in one small loop.

## Remaining Launch Risks

- Execution hardening risk: local-process execution is intentionally narrow and not sandboxed.
- Environment risk: no packaged runtime backend exists beyond the local host process model.
- Validator risk: current validators are useful but still shallow for scientific data integrity.
- Observer risk: domain-aware output interpretation is still mostly limited to the first `marker_report` slice.
- Cache risk: current hashing and explicit pruning are sufficient for preview demos, not long-term reproducibility or storage lifecycle guarantees.
- Product-positioning risk: the repo now has real execution and evidence primitives, but the full agentic planning/research layer is still missing.
- UX risk: the product currently expects operator familiarity with YAML, artifact IDs, and CLI-driven iteration.

## Launch Recommendation

Proceed with a technical preview only if release notes and demo scripts stay aligned with the current scope:

- emphasize CLI-first local runtime
- emphasize operator-controlled workflows
- avoid claims about autonomy, production hardening, or broad environment support
- keep the marker-demo path as the primary demonstration story

Supporting evidence already captured elsewhere:

- [v1-usable-slice-status.md](v1-usable-slice-status.md)
- [function-completion-audit-2026-05-29.md](function-completion-audit-2026-05-29.md)
- [phase2-state-layer-status-2026-05-29.md](phase2-state-layer-status-2026-05-29.md)
