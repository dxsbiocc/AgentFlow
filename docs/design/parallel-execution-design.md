# RFC: Parallel step execution (scheduler fan-out)

Status: IMPLEMENTED (Option A, behind `--max-parallel`)
Date: 2026-06-24
Relates to: agent-scheduling-design.md (#75), the multi-level chaining work (#95)

> Implemented per the Option A recommendation: `run_step` was split into
> `prepare_step` → (`run_local_command`) → `record_step`; `run_flow_with` runs a
> wave's subprocesses on scoped worker threads (`run_ready_wave_parallel`), at
> most `RunConfig.max_parallel` at a time, with preparation and recording serial
> on the main thread. `--max-parallel N` (default 0/1 = sequential,
> byte-identical) on `run` and `agent run`. A consistency test asserts a fan-out
> flow run in parallel yields byte-identical computed outputs to a serial run;
> a live demo showed 4 independent `sleep 1` steps drop ~4.0s → ~1.0s. The
> in-wave failure policy (stop the outer loop after a wave with any failure) and
> same-key cache dedupe remain as noted below.

## Problem

The `StepScheduler` (P2.1a, #76) *orders* ready steps but `run_flow_with`
executes them **sequentially**:

```rust
// crates/agentflow-core/src/runtime/mod.rs
loop {
    let ready = RuleBasedStepScheduler.order(ready_steps(...), &edges);
    if ready.is_empty() { break; }
    for step in ready {                 // ← sequential
        let attempt = self.run_step(flow_id, &step, config)?;
        ...
    }
}
```

When a flow fans out (several independent ready steps in one wave), they could
run concurrently. Today they don't. The existing invariant — *scheduling only
reorders execution, it never changes results* — must continue to hold for any
parallel version.

## Why it's not a quick change

`ProjectStore` owns a **single** `rusqlite::Connection`:

```rust
pub struct ProjectStore { root_path: PathBuf, conn: Connection }
```

`Connection` is `Send` but not `Sync`; all of `run_step`'s work — cache lookup,
env preparation, input staging, the tool subprocess, and writing back the
attempt / artifacts / lineage / observation — flows through `&self` and that one
connection. Naively spawning `run_step` on threads would race the connection.
So real parallelism needs one of the approaches below, each a non-trivial change
to the execution core. This is why it is **deferred**, not bundled with the
cheaper wins (#94/#95, answer-tool ranking).

## Options

### A. Split `run_step` into prepare → execute → record; parallelize execute only
- `prepare_step(&self, …) -> StepPlan` — all DB reads (cache check, env, staged
  input paths). Main thread.
- `execute_plan(plan) -> RawStepOutput` — spawn + wait on the tool subprocess.
  **No DB, no `&self`.** Runnable on a worker thread.
- `record_step(&self, plan, raw) -> AttemptSummary` — all DB writes. Main thread.

Then a wave becomes: prepare all (serial) → `execute` all in parallel (a scoped
thread pool, bounded by a `--max-parallel` knob) → record all (serial, in a
deterministic order). The DB is never touched concurrently, so the single
connection stays correct; only the I/O-bound subprocesses overlap.
- **Pros:** keeps the single connection; the slow part (subprocesses) parallelizes;
  determinism is easy (record in scheduler order); smallest blast radius on the
  storage layer.
- **Cons:** `run_step` is a large, cache/lineage/observer-entangled function;
  splitting it cleanly is the bulk of the work and risks subtle behavior drift.
  Cache *writes* still serialize (fine), but two steps that would write the same
  cache key in one wave can't share a write — acceptable (independent steps rarely
  collide).

### B. Connection pool (one connection per worker)
Give each worker its own `Connection` (WAL mode) from a pool; run full `run_step`
per worker thread.
- **Pros:** `run_step` stays whole.
- **Cons:** biggest change — `ProjectStore` is built around one owned connection
  and is threaded through ~100 call sites; WAL + busy-timeout + write contention
  needs care; determinism of interleaved writes is harder to guarantee. Highest
  risk to the honesty/lineage invariants.

### C. Do nothing (status quo) — sequential, scheduler-ordered
For typical local research flows the graph is mostly a dependency chain (#95 ladders
are linear), so realized parallelism is often 1. The performance win is modest and
the concurrency risk is real.

## Recommendation

**Defer until there is a driving use case** (a flow with a genuinely wide,
slow-step fan-out where wall-clock matters). When that arrives, implement **Option
A** behind a `--max-parallel N` flag (default 1 = today's behavior, byte-identical),
and lock the existing invariant with a test: the same flow run serially vs in
parallel must produce identical artifacts/observations/verdict, only faster. This
honors [[no-high-load-local-builds]] (opt-in, bounded) and the determinism
invariant. Option B is a fallback only if per-step DB work (not subprocesses)
becomes the bottleneck.

## Out of scope / open questions

- Cancellation + partial-failure semantics for an in-flight wave (today the loop
  stops on first failure; parallel needs a join-then-decide policy).
- Cache-key collisions within one wave (two independent steps, same key) — detect
  and dedupe at record time.
- Interaction with container backends (each step already isolated; parallel just
  means several `docker run`/subprocesses at once — bound by `--max-parallel`).
