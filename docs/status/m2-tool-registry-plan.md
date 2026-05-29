# M2 Tool Registry Implementation Plan

Status: Implemented
Date: 2026-05-28
Scope: V0 local tool registration, listing, and inspection

## 1. Purpose

M2 Tool Registry gives AgentFlow a durable boundary for tool capability metadata.

The goal is not to execute tools yet. The goal is:

> A project can register explicit local tool specs, list available tools, and inspect the stored version metadata through the CLI and core storage API.

This is the foundation for later flow validation and runtime execution.

## 2. Scope

M2 implements:

- V0 tool spec metadata parsing.
- Tool registration into `tools` and `tool_versions`.
- Stable tool refs using `namespace/name`.
- Version-specific refs using `namespace/name@version`.
- Tool listing.
- Tool inspection.
- Versioned JSON output for CLI consumers.
- A first example tool spec for the tumor marker V0 scenario.

M2 does not implement:

- runtime execution
- flow validation
- command template expansion
- input/output type validation
- automatic environment checks
- tool discovery
- tool catalog search
- network lookup
- package installation
- Research Mode
- Omiga adapter
- Docker, Singularity, Nextflow, or remote execution

## 3. Tool Spec Parsing Decision

M2 uses a small V0-only simple YAML metadata parser instead of adding a full YAML dependency.

Reason:

- The workspace guidance discourages new dependencies without explicit request.
- M2 only needs top-level metadata: `schema_version`, `namespace`, `name`, `version`, `maturity`, and `description`.
- Nested `inputs`, `params`, `outputs`, `runtime`, `validators`, and `observer` are preserved in the original source text for later validator work.

This is intentionally temporary. A later flow-validation milestone can introduce structured parsing after the tool contract stabilizes.

## 4. CLI Surface

Implemented:

```text
agentflow tools register <tool.yaml> [--path <path>]
agentflow tools list [--json] [--path <path>]
agentflow tools inspect <tool-ref> [--json] [--path <path>]
```

Tool refs:

```text
marker/marker_survival_scan
marker/marker_survival_scan@0.1.0
```

If namespace is omitted during inspection, `local` is assumed.

## 5. Storage Behavior

Registration writes:

- `tools`
- `tool_versions`
- `events`

Rules:

- Re-registering the same `namespace/name@version` updates that version record.
- Registering a new version updates `tools.latest_version`.
- Tool specs are stored as canonical JSON containing parsed metadata plus the original source text.
- The spec hash is computed from the stored JSON payload.

## 6. Review Gate

Automatic gate:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Manual CLI gate:

```text
agentflow init --name ToolDemo --path <temp>
agentflow tools register examples/tools/marker_survival_scan.tool.yaml --path <temp>
agentflow tools list --json --path <temp>
agentflow tools inspect marker/marker_survival_scan --json --path <temp>
```

## 7. Residual Risk

- The simple parser is not a full YAML parser.
- Nested input/output/runtime schema is not validated yet.
- Tool specs do not yet prove the declared command exists.
- The registry does not install dependencies or discover external tools.
- Later structured validation must not silently reinterpret existing stored specs without migration rules.
