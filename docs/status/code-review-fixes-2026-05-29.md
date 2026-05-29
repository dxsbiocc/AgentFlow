# Code Review Fixes

Date: 2026-05-29
Scope: Full current code review after the function-completion audit.

## Fixed

1. `cache explain <step-id>` was advertised but not implemented.
   - Added `ProjectStore::cache_explain_target`.
   - Added `ProjectStore::cache_explain_step_ref`.
   - Reused step-ref forms supported by retry: `flow.step`, `flow/step`, `step:flow/step`, and unique local step id.
   - Updated CLI to call the target resolver instead of flow-only explanation.

2. `cache explain` could fail before upstream `step.output` artifacts existed.
   - Flow-level explanation now returns a miss with `cache_key: unavailable` and a concrete reason instead of failing the whole command.

3. CLI status text was stale.
   - Replaced V0/M5 wording with V1 usable CLI runtime wording.

4. Runtime output path naming allowed exact `.` or `..` path parts.
   - Output aliases and output names now pass through a safer path-part sanitizer.

5. `command.sh` display could misrepresent argv containing spaces or punctuation.
   - Added shell-style quoting for materialized command display.

6. `read_logs` silently swallowed missing log files.
   - Missing stdout/stderr files now surface as I/O errors instead of empty logs.

7. Existing projects were opened without applying idempotent migrations.
   - `ProjectStore::open` now applies migrations, improving upgrade compatibility.

8. Hand-written JSON map readers broke on commas, colons, and escaped quotes inside strings.
   - Hardened runtime/report map parsing.
   - Hardened stored tool spec string-map extraction.
   - Added regression tests for punctuation-bearing map values.

9. Documentation contained now-stale cache explanation limitations.
   - Updated V1 status, CLI demo slice status, acceptance plan, and completion audit.

## Verification

```text
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -q -p agentflow-cli -- cache explain marker_demo.scan --path /private/tmp/agentflow-v1-demo-20260529115719
cargo run -q -p agentflow-cli -- help
```

Observed:

- `agentflow-cli`: 13 tests passed.
- `agentflow-core`: 43 tests passed.
- `agentflow-schemas`: 3 tests passed.
- Clippy passed with warnings denied.
- Step-level cache explain returned `step:marker_demo/scan [hit]`.

## Remaining Risks

- Retry still creates a new run/attempt pair rather than appending another attempt under the same prior run record.
- Hashing remains FNV-64 for the no-new-dependency MVP; SHA-256 is still the right future cache/integrity direction.
- YAML parsing remains intentionally small and V0-shaped; a structured parser should replace it before richer specs.
- Local execution still lacks container/sandbox/resource limits.
- Agent planning, observer records, graph patching, Research Mode, and Omiga adapter remain out of this runtime slice.
