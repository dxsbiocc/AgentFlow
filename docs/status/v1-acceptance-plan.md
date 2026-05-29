# V1 Acceptance Plan

Status: Updated after V1 usable slice implementation
Date: 2026-05-29
Scope: First usable acceptance matrix based on completed M0-M5 capabilities

## 1. Purpose

This plan defines the first usable acceptance bar for AgentFlow after M5 runtime execution.

It is intentionally evidence-driven:

- accept only behavior that is implemented today
- distinguish automated release gates from manual demo proof
- mark missing V1-scope surfaces as blockers instead of assuming them

The current implemented CLI surface is:

```text
init
status
doctor
tools register/list/inspect
import
artifacts list/inspect
flow validate/approve/inspect
run
run-step
logs
cache explain
retry
report
```

## 2. Acceptance Position

For the first usable release cut:

- `init/register/import/approve/run/logs/artifacts/status` must pass automated acceptance.
- `cache` must have automated regression coverage and one manual CLI demo.
- `run-step` is acceptable as selected-step execution with dependency gating and completed-step rerun rejection.
- `retry` is acceptable for failed-step retry/requeue semantics, with richer policy deferred.
- `report` is acceptable as a Markdown provenance report, with JSON/export formats deferred.
- `patch apply` is acceptable for `add_step`, constrained `add_edge`, and `update_params` with downstream invalidation; richer mutation semantics are deferred.

## 3. Acceptance Matrix

| Capability | Success path to prove | Failure path to prove | Gate type | Current status |
| --- | --- | --- | --- | --- |
| `init` | creates project DB and usable project root | rejects/flags non-project usage through follow-up `doctor` or `status` on wrong path | Must automate | Ready |
| `register` | valid tool spec registers and is inspectable | invalid tool spec or missing executable contract is rejected | Must automate | Ready |
| `import` | reference and copy imports register artifacts with metadata | missing file, invalid mode, or malformed args are rejected | Must automate | Ready |
| `approve` | valid flow validates and approves into storage | unknown tool, missing artifact, unknown dependency, cycle, missing required input, unknown port/param are rejected | Must automate | Ready |
| `run` | approved flow executes local tool, writes workdir, captures stdout/stderr, registers computed outputs | unapproved flow rejected; command non-zero or missing output marks step failed and preserves logs | Must automate | Ready |
| `run-step` | selected draft/ready/failed step executes after dependencies are complete | incomplete dependencies or completed-step rerun are rejected | Must automate | Ready for selected-step scope |
| `logs` | logs readable by attempt id and latest attempt of run id | unknown run/attempt id is rejected | Must automate | Partially evidenced, add explicit unknown-id acceptance case |
| `artifacts` | imported and computed artifacts list/inspect are stable and machine-readable | unknown artifact id is rejected | Must automate | Partially evidenced, add explicit unknown-id acceptance case |
| `status` | JSON is stable and counts reflect flows, runs, attempts, artifacts | wrong/non-project path is rejected | Must automate | Ready |
| `cache` | second identical flow produces `cache_hit` attempt and readable evidence without rerunning command payload | stale, changed, or incomplete cache state must fail safely, not silently succeed | Auto regression + manual demo | Ready |
| `retry` | reruns failed step as new attempt without deleting old attempt/logs | invalid retry target or retry of non-failed step is rejected | Must automate | Ready for failed-step scope |
| `report` | generates stable markdown report from run/artifact evidence | invalid flow/report target is rejected | Auto regression + manual demo | Ready for Markdown scope |
| `patch apply` | approved add_step/update_params patches change executable graph state and invalidate downstream steps for replay | unapproved patch, invalid params, running-step invalidation, or unsafe existing-edge rewrite is rejected | Must automate | Ready for narrow graph mutation scope |

## 4. What Must Be Automated

These are release blockers if only shown in a human demo:

- `init` success path
- `register` success and invalid-spec failure
- `import` reference success, copy success, missing-file failure
- `approve` success and validation failures
- `run` success and failed-command failure
- `run-step` selected-step success, incomplete-dependency rejection, and completed-step rerun rejection
- `logs` success and unknown-id failure
- `artifacts` list/inspect success and unknown-id failure
- `status --json` schema stability and count updates
- `cache` second-run `cache_hit` regression
- `patch apply` add_step success, update_params success, validation rejection, and running-step no-partial-update rejection

Rationale:

- these behaviors are deterministic CLI/runtime state transitions
- they are likely to regress during storage/runtime refactors
- they define whether AgentFlow is actually usable without Omiga

## 5. What Can Stay Manual For Now

Manual demo is sufficient only for:

- end-to-end happy-path storytelling across multiple commands
- human readability of logs output
- human readability of generated Markdown-like artifacts produced by tools
- richer report export formats such as JSON, HTML, or file materialization
- richer cache operations such as list, prune, and structured miss diagnostics

Manual demo is not sufficient for future agent planning, research mode, or branching signoff because those surfaces are not in this V1 runtime slice.

## 6. Minimal Acceptance Script Sequence

Use two sequences: one happy path, one failure path.

### Sequence A: happy path plus cache

```text
TMP="$(mktemp -d)"
cargo run -p agentflow-cli -- init --name V1Demo --path "$TMP"
cargo run -p agentflow-cli -- tools register examples/tools/marker_survival_scan.tool.yaml --path "$TMP"
cargo run -p agentflow-cli -- import examples/data/expression.tsv --type TSV --path "$TMP"
cargo run -p agentflow-cli -- import examples/data/survival.tsv --type TSV --path "$TMP"
cargo run -p agentflow-cli -- flow approve <flow-with-imported-artifact-ids>.yaml --path "$TMP"
cargo run -p agentflow-cli -- run marker_demo --path "$TMP"
cargo run -p agentflow-cli -- cache explain marker_demo --path "$TMP"
cargo run -p agentflow-cli -- report marker_demo --path "$TMP"
cargo run -p agentflow-cli -- status --json --path "$TMP"
cargo run -p agentflow-cli -- artifacts list --json --path "$TMP"
cargo run -p agentflow-cli -- logs <attempt-id-from-first-run> --path "$TMP"
cargo run -p agentflow-cli -- flow approve <second-equivalent-flow-with-imported-artifact-ids>.yaml --path "$TMP"
cargo run -p agentflow-cli -- run marker_demo_cached --path "$TMP"
cargo run -p agentflow-cli -- logs <cache-hit-attempt-id> --path "$TMP"
```

Acceptance assertions:

- first run completes with computed artifact output
- `status --json` shows non-zero `flows`, `runs`, `run_attempts`, and `artifacts`
- first run logs show real command execution output
- equivalent second flow records `cache_hit`
- cache-hit attempt logs contain cache-hit evidence
- report includes flow, steps, attempts, referenced inputs, produced outputs, and failures

### Sequence B: validation and runtime failures

```text
TMP="$(mktemp -d)"
cargo run -p agentflow-cli -- init --name V1FailureDemo --path "$TMP"
cargo run -p agentflow-cli -- flow approve examples/flows/marker_demo.flow.yaml --path "$TMP"
```

Expected:

- fails because required tools/artifacts are not prepared

Then create a local failing tool and run it:

```text
cat > "$TMP/fail.tool.yaml" <<'EOF'
schema_version: agentflow.tool.v0
namespace: marker
name: failing_scan
version: 0.1.0
maturity: wrapped
description: Fail deliberately
inputs:
  expression_table:
    type: TSV
    required: true
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/sh
    - /absolute/path/to/failing-script.sh
EOF

cat > "$TMP/fail.flow.yaml" <<'EOF'
schema_version: agentflow.flow.v0
id: failing_demo
name: Failing demo
steps:
  - id: scan
    tool: marker/failing_scan
    reason: Prove failed attempts retain logs
    needs: []
    inputs:
      expression_table: artifact_expression
    outputs:
      report: marker_report
EOF
```

Before approving `fail.flow.yaml`, replace `artifact_expression` with the real imported artifact id from:

```text
cargo run -p agentflow-cli -- import examples/data/expression.tsv --type TSV --path "$TMP"
```

Then run:

```text
cargo run -p agentflow-cli -- tools register "$TMP/fail.tool.yaml" --path "$TMP"
cargo run -p agentflow-cli -- flow approve "$TMP/fail.flow.yaml" --path "$TMP"
cargo run -p agentflow-cli -- run failing_demo --path "$TMP"
cargo run -p agentflow-cli -- logs <attempt-id> --path "$TMP"
```

Acceptance assertions:

- failed run returns failed step count
- logs preserve both stdout and stderr
- step status becomes `failed`
- no silent success is possible when command exits non-zero
- `agentflow retry failing_demo.scan --path "$TMP"` appends another failed attempt

## 7. Must-Block Bug Types

These defects should block the first usable release immediately:

- flow runs without prior approval
- required input/param/output contract violations are accepted
- runtime reports success when declared outputs are missing or empty
- failed command loses stdout/stderr or overwrites prior attempt logs
- selected-step execution bypasses unfinished upstream dependencies
- completed-step rerun happens silently without an explicit graph/state change
- parameter patch changes a completed step without invalidating downstream replay
- running-step invalidation partially updates params before rejecting
- workdir reuse causes one attempt to overwrite another
- downstream `step.output` resolves to the wrong artifact
- `status --json` schema or count fields regress incompatibly
- computed artifacts are not registered or point to wrong paths
- cache hit returns stale/wrong artifact for a changed input set
- cache hit is reported without actually validating/restoring outputs
- CLI accepts unknown ids and returns misleading success
- retry, once implemented, mutates or deletes prior attempts instead of appending a new one
- report, once implemented, omits provenance of inputs/runs/artifacts

## 8. Release Decision

Current evidence supports the V1 usable CLI runtime slice for:

- `init/register/import/approve/run/logs/artifacts/status`
- `run-step` for selected-step execution
- `cache explain`
- `retry` for failed-step reruns
- `report` as Markdown provenance output
- narrow `patch apply` graph mutation for add_step/update_params

Current evidence does not support these later product claims yet:

- Agent planning or Research Mode
- richer dynamic graph mutation beyond add_step/update_params and narrow branching
- Conda/Docker/Singularity execution backends
- Omiga UI integration
- full redaction/sandbox policy

So the honest release framing today is:

> AgentFlow has a first usable CLI-first local runtime slice. It is not yet the full agentic research workflow product.
