# M4 Flow Validate Implementation Plan

Status: Implemented
Date: 2026-05-28
Scope: V0 static flow parsing, validation, approval, and inspection

## 1. Purpose

M4 connects registered tools and imported artifacts into a static DAG before runtime execution exists.

The goal is:

> A user can describe a simple scientific flow, validate tool/artifact references and step dependencies, approve it into the project database, and inspect the stored graph.

This creates the input contract for the later runtime scheduler.

## 2. Scope

M4 implements:

- `agentflow.flow.v0` simple YAML parsing.
- flow-level `id` and `name`.
- step-level `id`, `tool`, `needs`, `reason`, `inputs`, `params`, and `outputs`.
- tool reference validation against the Tool Registry.
- artifact reference validation against imported artifacts.
- simple `step.output` input references.
- duplicate step detection.
- unknown dependency detection.
- dependency cycle detection.
- flow approval into `flows`, `steps`, and `edges`.
- flow inspection as versioned JSON.

M4 does not implement:

- runtime execution
- workdir creation
- cache keys
- output artifact materialization
- tool input/output type compatibility
- environment checks
- Agent planning
- graph mutation
- Research Mode
- Omiga adapter

## 3. Flow DSL Decision

M4 uses a narrow simple YAML parser rather than a full YAML parser.

Supported shape:

```yaml
schema_version: agentflow.flow.v0
id: marker_demo
name: Marker demo
steps:
  - id: scan
    tool: marker/marker_survival_scan
    reason: Evaluate TP53 marker signal
    needs: []
    inputs:
      expression_table: artifact_...
    params:
      gene: TP53
    outputs:
      report: marker_report
```

Input references can be:

```text
artifact:<id>
artifact_<id>
step_id.output_name
```

This is enough for M4 validation and avoids adding a parsing dependency before the flow contract stabilizes.

## 4. CLI Surface

Implemented:

```text
agentflow flow validate <flow.yaml> [--json] [--path <project>]
agentflow flow approve <flow.yaml> [--path <project>]
agentflow flow inspect <flow-id> [--json] [--path <project>]
```

## 5. Storage Behavior

Approval writes:

- `flows`
- `steps`
- `edges`
- `events`

Rules:

- `flow validate` does not write to storage.
- `flow approve` refuses invalid flows.
- `flow approve` refuses to overwrite an existing approved flow id.
- Stored step ids are database-scoped as `step:<flow_id>/<local_step_id>`.
- Stored step status starts as `draft`.

## 6. Review Gate

Automatic gate:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Manual CLI gate:

```text
agentflow init --name FlowDemo --path <temp>
agentflow tools register examples/tools/marker_survival_scan.tool.yaml --path <temp>
agentflow import examples/data/expression.tsv --type TSV --path <temp>
agentflow flow validate <flow.yaml> --json --path <temp>
agentflow flow approve <flow.yaml> --path <temp>
agentflow flow inspect marker_demo --json --path <temp>
```

## 7. Residual Risk

- The parser is not a full YAML parser.
- Tool input/output type compatibility is not checked yet.
- `step.output` references only check producer step existence, not declared output names.
- Approved flows are immutable for now; replacement/superseding needs a later design.
- Runtime may need to promote steps from `draft` to `ready` after stronger validation.
