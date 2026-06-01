# AgentFlow

AgentFlow is a CLI-first local workflow runtime for scientific task graphs, with an
**agent control layer** that drives the research loop: hypothesis → evidence → three-state
verdict → branch selection → human handoff, with a full audit trail.

Posture as of 2026-06-01:

- The runtime slice (tool/artifact/flow/run/cache/env) and the agent control layer
  (four engines + propose-mode control loop + literature retrieval) are implemented and
  green on the workspace baseline.
- The control loop runs in **propose mode**: it advances autonomously but proposes graph
  changes and raises decision points rather than auto-applying — autonomous apply is gated
  pending explicit enablement (see "Explicitly Not Supported Yet").
- Repo-local and Rust-workspace driven; no packaged binary release yet.

See [docs/agentflow-agent-control-layer-design.md](docs/agentflow-agent-control-layer-design.md)
for the control-layer architecture (engines, control constitution A1–A4, milestones H1–H7a),
and [launch-readiness-2026-05-29.md](docs/status/launch-readiness-2026-05-29.md) for the
runtime-slice launch status.

See [launch-readiness-2026-05-29.md](docs/status/launch-readiness-2026-05-29.md) for the launch-facing status, known gaps, and residual risks.

## Quick Start

Current entrypoint:

```bash
cargo run -q -p agentflow-cli -- help
```

Create a demo project:

```bash
export AF_DEMO=/private/tmp/agentflow-tech-preview-demo
rm -rf "$AF_DEMO"

cargo run -q -p agentflow-cli -- init --name TechPreviewDemo --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- tools register examples/tools/marker_survival_scan.tool.yaml --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- import examples/data/expression.tsv --type TSV --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- import examples/data/survival.tsv --type TSV --path "$AF_DEMO"
```

Then:

1. Inspect the imported artifact IDs with `agentflow artifacts list --json`.
2. Copy `examples/flows/marker_demo.flow.yaml` into your demo directory.
3. Replace `artifact_REPLACE_WITH_IMPORTED_ID` and `artifact_REPLACE_WITH_IMPORTED_SURVIVAL_ID` with the imported artifact IDs.
4. Validate, approve, and run the flow.

Example:

```bash
cp examples/flows/marker_demo.flow.yaml "$AF_DEMO/marker_demo.flow.yaml"

cargo run -q -p agentflow-cli -- flow validate "$AF_DEMO/marker_demo.flow.yaml" --json --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- flow approve "$AF_DEMO/marker_demo.flow.yaml" --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- run marker_demo --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- status --json --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- report marker_demo --path "$AF_DEMO"
```

## Supported In This Technical Preview

- Project lifecycle: `init`, `status`, `doctor`
- Tool registry: `tools register`, `tools list`, `tools inspect`
- Artifact registry: reference/copy import, list, inspect
- Flow lifecycle: `flow validate`, `flow approve`, `flow inspect`
- Execution: approved local DAG execution with `run`
- Targeted execution: `run-step` with dependency gating
- Retry: `retry` for failed steps while preserving attempt history
- Run management visibility: `runs list`, `runs inspect`, and `logs`
- Cache/resume slice: deterministic cache keys, cache-hit restore, `cache explain`, cache inventory, and explicit cache prune
- Reports: Markdown `report`
- Observation/state layer: `observe`, `observations *`, `research *`, `patch *`, `compare *`
- Runtime-connected output observation for declared observers such as `marker_report`
- Deterministic table-oriented validation for `required_columns`, `min_rows`, cross-input `sample_id_column`, input `profile`, and tool-level `validator_profile`
- Local runtime timeout control through `runtime.timeout_seconds`
- Existing Conda/micromamba environment execution through explicit `runtime.runner` plus `env_name` or `env_prefix`
- Environment readiness checks through `env check <tool-ref>`
- Explicit Conda/micromamba environment update through `env prepare <tool-ref>` when `runtime.env_file` is declared
- Conda/micromamba environment export evidence through `env export <tool-ref>`, including export hash and conservative package-set diff against declared `runtime.env_file` dependencies

## Agent Control Layer

The control layer turns the runtime into a research-reasoning loop. All state is event-sourced
into the same project-local `.agentflow/` store; engines never bypass the runtime, never mutate
the database or shell directly, and the loop never auto-applies graph changes.

- **Argument engine** — hypotheses as first-class objects with a 7-state lifecycle, an evidence
  ledger graded by evidence quality, and a rule-based three-state verdict (`affirmed` / `refuted` /
  `inconclusive`, the latter split into provisional vs fundamental). Strong verdicts require a
  self-deception gate (against + alternatives + non-speculative basis) before they can be recorded.
  - `hypothesis create|list|show|transition`, `evidence link|list`, `verdict render|show`
- **Branch engine** — verdict-driven branch selection (deepen / spawn / abandon / hold) with
  deterministic scoring; proposes graph patches through the approval-gated patch flow only.
  - `branch candidates|select`
- **Handoff engine** — brake policy that hands control back to the user at high-cost / irreversible /
  goal-mutating forks; decision points carry a digest, options, and a recommendation.
  - `decision list|pending|show|resolve`
- **Forage engine** — literature retrieval with §15 access-status compliance (abstract-only evidence
  cannot drive an `affirmed` verdict); freshness/evaporation modelling. Retrieval runs as an external
  process so the Rust core stays HTTP/dependency-free.
  - `forage fetch|ingest|observe|list|show|link` (real PubMed via `examples/tools/pubmed_search.py`)
- **Trace safety net** — checkpoints, cumulative-drift detection, and append-only revert records that
  make autonomous advancement auditable and reversible.
  - `trace checkpoint|list|drift|revert`
- **Control loop** — `agent run` orchestrates the engines for one cycle: previews verdicts, persists
  provisional ones, and raises a decision point (instead of fabricating a claim) whenever the evidence
  would imply a strong verdict that needs a human self-deception gate.

End-to-end research loop:

```bash
agentflow forage fetch --query "KRAS G12C resistance" --max 10 --path "$AF_DEMO"
agentflow hypothesis create --statement "KRAS G12C resistance is adaptive" --origin user_goal --goal g1 --path "$AF_DEMO"
agentflow forage list --json --path "$AF_DEMO"            # copy a forage observation id
agentflow forage link --hypothesis <hyp-id> --observation <forage-obs-id> --stance supports --note "PubMed" --path "$AF_DEMO"
agentflow agent run --path "$AF_DEMO"                     # autonomous cycle; hands off on strong verdicts
agentflow decision pending --path "$AF_DEMO"              # resolve raised decisions
```

## Explicitly Not Supported Yet

- Autonomous graph mutation: the control loop (`agent run`) proposes branch patches and raises decision points but does **not** auto-apply graph changes or auto-transition hypotheses; auto-apply within a safe envelope (and revert-horizon-aware projections) is gated pending explicit enablement
- Automatic tool recommendation/selection driven by forage results
- Implicit environment creation, solving, or package installation during `run`
- Full lockfile normalization, dependency solving, package-manager-specific diff semantics, or environment garbage collection
- Remote or isolated execution backends such as Docker, Singularity, or SLURM
- Parallel scheduler execution or cancellation controls
- Rich semantic validators such as file signatures, domain-specific QC policies, and pluggable validator registries
- Full graph-branch lifecycle such as delete, merge, rollback, supersede, or decision-node management
- Cache eviction policy beyond explicit `--all` and `--older-than-seconds` pruning
- JSON/HTML report export or persisted report artifacts
- Full-text retrieval, citation graphs, Unpaywall resolution, or non-PubMed literature sources (forage currently covers PubMed metadata/abstracts via an external script)
- Security sandboxing, container isolation, redaction policy, or resource quotas

## Verification Commands

Use the acceptance gate before treating a branch as launch-candidate documentation:

```bash
scripts/acceptance-v1.sh
```

The script runs the Rust gates and a CLI marker-demo smoke test. The individual gates are:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -q -p agentflow-cli -- help
```

For the demo flow, also verify:

```bash
cargo run -q -p agentflow-cli -- status --json --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- logs run_attempt_... --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- runs list --flow marker_demo --json --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- artifacts list --json --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- observations list --json --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- cache explain marker_demo.scan --path "$AF_DEMO"
cargo run -q -p agentflow-cli -- cache list --json --path "$AF_DEMO"
```

## Example Operator Flow

The current happy-path operator workflow is:

1. Register an executable tool spec with declared inputs, outputs, validators, and local runtime argv.
2. Import input artifacts into a project-local `.agentflow/` state directory.
3. Validate and approve a static flow YAML.
4. Run the approved flow locally.
5. Inspect status, logs, computed artifacts, and the generated Markdown report.
6. Use `run-step`, `retry`, `patch`, `observe`, and `compare` only as operator-driven follow-up controls.

## Remaining Launch Risks

- Local execution is intentionally narrow and still lacks sandbox/container hardening.
- Cache hashing remains lightweight; pruning is explicit but does not yet include policies or artifact garbage collection.
- Validation and observer coverage are deliberately narrow, especially outside the `marker_report` demo path.
- The runtime is strong enough for a technical preview, but still materially short of the broader AgentFlow product vision.
