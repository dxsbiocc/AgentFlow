# AgentFlow

AgentFlow is a CLI-first local workflow runtime for scientific task graphs.

Technical preview posture as of 2026-05-29:

- Scope is ready for local, operator-driven demos of the usable CLI runtime slice on a green workspace baseline.
- Not ready to be positioned as a full autonomous research workflow system.
- Current implementation is repo-local and Rust-workspace driven; there is no packaged binary release yet.
- This branch should only be published after the Rust workspace gates below are green.

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

## Explicitly Not Supported Yet

- Agent planning, tool recommendation, or autonomous graph authoring
- Remote execution backends such as Conda, Docker, Singularity, or SLURM
- Parallel scheduler execution or cancellation controls
- Rich semantic validators such as file signatures, domain-specific QC policies, and pluggable validator registries
- Full graph-branch lifecycle such as delete, merge, rollback, supersede, or decision-node management
- Cache eviction policy beyond explicit `--all` and `--older-than-seconds` pruning
- JSON/HTML report export or persisted report artifacts
- Literature search, citation tracking, or networked Research Mode
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
