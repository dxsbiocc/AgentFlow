# M1 Contracts Review Record

Status: Automatic gate passed; manual review passed; review subagent blocked
Date: 2026-05-28
Scope: Domain and schema contracts only

## Implemented

- V0 step status enum.
- V0 run attempt status enum.
- V0 tool maturity enum.
- V0 artifact kind enum.
- Step state transition guard.
- V0 table-name contract.
- Deferred table-name contract.

## Scope Check

This slice intentionally does not implement:

- SQLite migrations
- repository layer
- runtime execution
- CLI storage commands
- artifact import
- tool registration
- Research Mode
- Omiga adapter

## Automatic Gate Evidence

Passed:

```text
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Tests now cover:

- schema names are versioned
- V0 table names match MVP scope
- deferred tables do not leak into V0 table contract
- step status names match V0 contract
- step status transitions allow only V0 legal transitions
- run attempt status names match V0 contract

## Review Subagent Status

Review automation is currently unreliable. Three M0 review attempts timed out before this slice. This M1 contract slice is therefore recorded as:

> Passed automatic gate, pending human or later subagent review.

Manual review update:

> Passed human review on 2026-05-28 with no blocking findings. Contracts remain scope-limited: no SQLite implementation, runtime execution, Omiga adapter, Research Mode, Docker, or Nextflow behavior is present.

## Residual Risk

- The contracts are not yet connected to persistence or CLI behavior.
- A later storage implementation must not redefine these strings or bypass the state transition guard.
- Deferred-table constants exist only as guardrails; future implementation must not treat them as V0 storage scope.
