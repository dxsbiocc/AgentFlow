# M2 Tool Registry Review Record

Status: Automatic gate passed; manual CLI gate passed; subagent review not rerun
Date: 2026-05-28
Scope: V0 tool registry only

## Implemented

- `ToolSpec::from_simple_yaml` for top-level V0 metadata.
- `ProjectStore::register_tool`.
- `ProjectStore::list_tools`.
- `ProjectStore::inspect_tool`.
- `agentflow tools register`.
- `agentflow tools list`.
- `agentflow tools inspect`.
- Versioned JSON schemas for tool list and tool inspection outputs.
- Example spec: `examples/tools/marker_survival_scan.tool.yaml`.

## Scope Check

This slice intentionally does not implement:

- flow validation
- runtime execution
- artifact import
- cache behavior
- input/output type checking
- command existence checking
- Research Mode
- web or literature lookup
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

- V0 tool maturity parsing.
- simple tool metadata parsing.
- wrong tool schema rejection.
- tool registration.
- tool listing.
- tool inspection.
- latest-version update.
- same-version replacement.
- CLI register/list/inspect.

## Manual CLI Evidence

Passed in a temp project directory:

```text
cargo run -p agentflow-cli -- init --name ToolDemo --path /private/tmp/agentflow-m2-tools.zVR5k7
cargo run -p agentflow-cli -- tools register examples/tools/marker_survival_scan.tool.yaml --path /private/tmp/agentflow-m2-tools.zVR5k7
cargo run -p agentflow-cli -- tools list --json --path /private/tmp/agentflow-m2-tools.zVR5k7
cargo run -p agentflow-cli -- tools inspect marker/marker_survival_scan --json --path /private/tmp/agentflow-m2-tools.zVR5k7
```

Observed result:

- `register` returned `marker/marker_survival_scan`.
- `list --json` returned `agentflow.tool_list.v0`.
- `inspect --json` returned `agentflow.tool_inspection.v0`.

## Review Notes

No blocking findings from direct manual review.

The largest intentional limitation is parser depth: M2 reads only top-level metadata and stores the original source text for later structured validation. This is acceptable because M2's product purpose is registry persistence, not execution safety.

## Residual Risk

- The parser will reject or misread complex YAML constructs.
- Since nested schema is not validated, a tool can be registered even if its declared inputs or runtime fields are malformed.
- Later runtime code must not treat registration as proof that a tool is executable or safe.
- A future dependency on a real YAML parser should be introduced only when flow/tool validation reaches that milestone.
