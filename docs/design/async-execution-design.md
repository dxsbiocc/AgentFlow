# Design: asynchronous / detached execution (submit → poll → collect)

Status: design + phased plan. Phase 1 (persistence substrate) is the first
implementation slice. `argument.rs` (the 0-LLM verdict engine) is untouched by
all phases — this is execution-layer only.

## 1. Motivation

Execution is synchronous today: `prepare_step` builds a `Command`, then
`run_local_command` spawns the subprocess and **waits** for it to exit before
recording the attempt. For a long-running job — an HPC submission, a Nextflow
pipeline, a multi-hour analysis — this blocks the whole AgentFlow process for the
job's entire duration, and a crash loses the in-flight work. There is no way to
**submit** a job, return, and **collect** it later.

Goal: let a step run **detached** — submit the job to an external runner (get a
job handle immediately), persist the handle, return; a later `run` (or explicit
`jobs poll`) polls the handle and, once the job is terminal, collects outputs and
finalizes the attempt. This is the missing piece of the north-star execution
model (agent-scheduled, per-tool isolated, long-running/HPC-capable).

## 2. The abstraction

A step's execution is one of:

- **Synchronous (today):** spawn a subprocess, wait, record. Used by
  `local` / `conda` / `micromamba` / `isolated-micromamba` / `container` /
  `nextflow` — the launcher process blocks until the work finishes.
- **Detached (new):** SUBMIT the job to an external runner that returns a handle
  and keeps running after the submit command exits; record the attempt as
  `Submitted` with the handle; RETURN. Later POLL the runner by the handle; when
  it reports a terminal status, COLLECT the declared outputs from the step
  workdir (the external job wrote them there) and finalize the attempt
  (`Succeeded` / `Failed`), reusing the existing output-validation + cache
  machinery.

## 3. Backend protocol (scheduler-agnostic core)

Detached execution is opted into by the tool's runtime, which declares a
**submit** command and a **poll** command. The core never contains SLURM /
Nextflow / cloud specifics — those live in the user's submit/poll scripts,
exactly like forage keeps network in user scripts. Proposed shape
(`runtime.backend: detached`):

```yaml
runtime:
  backend: detached
  submit:            # runs in the prepared step workdir with the AGENTFLOW_* env;
    - /abs/submit.sh # must print exactly one line "job_handle=<opaque-id>" and exit 0
  poll:              # invoked as: <poll...> <job_handle>
    - /abs/poll.sh   # must print one line "status=running|succeeded|failed" and exit 0
```

- `submit` prepares nothing extra — the core has already staged inputs and set
  `AGENTFLOW_INPUT_*/PARAM_*/OUTPUT_*/WORKDIR`; the script submits the real job
  (e.g. `sbatch`, `nextflow -bg`, a cloud API) so it writes its outputs to the
  same `AGENTFLOW_OUTPUT_*` paths, and echoes the handle.
- `poll <handle>` maps the external status to `running|succeeded|failed`.
- On `succeeded`, the core validates declared outputs from the workdir and
  finalizes exactly as a synchronous success; on `failed`, it records a failure.

A trivial **local detached** fixture (submit backgrounds a process and prints its
PID; poll checks the PID) is enough for tests and a demo; real SLURM/Nextflow is a
user script wrapping the scheduler.

## 4. State model

- New `RunAttemptStatus::Submitted` — a job submitted and running detached
  (terminal-pending, neither success nor failure).
- New `run_attempts.job_handle TEXT` column (migration **v3**:
  `ALTER TABLE run_attempts ADD COLUMN job_handle TEXT`). `V0_SCHEMA_SQL` is NOT
  modified (its checksum is validated); this is a new migration like the v2
  `modules` table.
- The `runs.status` for a submitted step's run is likewise a non-terminal
  "running" so it is not re-submitted and blocks dependents.

## 5. Run-loop integration

`run_flow_with` gains a **poll phase** at the top of each wave:

1. **Poll** every outstanding `Submitted` attempt in the flow: run the tool's
   `poll` command with the stored `job_handle`. `succeeded` → collect outputs +
   finalize `Succeeded` (+ cache); `failed` → finalize `Failed`; `running` →
   leave it.
2. **Advance** ready steps as today, except a ready **detached-backend** step is
   *submitted* (record a `Submitted` attempt with the handle) instead of run —
   the loop does not wait on it.
3. A `Submitted` step is excluded from `ready_steps` (its status is
   `submitted`, not `draft`/`ready`/`failed`) and is not in `completed_step_ids`,
   so it correctly **blocks dependents** and is not re-submitted.

Termination: the wave loop ends when there are no ready steps. Within one `run`
invocation, if steps remain `Submitted`, `run` returns and reports "N steps still
running (detached) — re-run to collect." So **`run` becomes resumable**:
re-running polls the outstanding jobs and advances. (A `--wait` mode that
poll-loops with a backoff until all jobs finish can come later.)

## 6. CLI

- `run <flow>` polls outstanding jobs, advances, and reports how many steps are
  still detached.
- `jobs list [--flow <id>]` — outstanding submitted attempts (step, handle,
  submitted_at). `jobs poll [--flow <id>]` — poll + finalize without advancing.

## 7. Phasing (each a small, reviewed PR)

- **Phase 1 — persistence substrate (FIRST codex task).** `RunAttemptStatus::Submitted`;
  migration v3 (`job_handle` column); storage methods:
  `record_submitted_attempt` (write a Submitted attempt carrying the handle),
  `outstanding_submitted_attempts(flow_id)` (list handle + workdir + step + tool),
  and `finalize_submitted_attempt(attempt_id, status, exit/error)` (transition a
  Submitted attempt to a terminal status). NO run-loop or backend change. Pure +
  unit-tested. Low risk, foundational.
- **Phase 2 — detached backend + protocol.** Parse `runtime.backend: detached`
  with `submit`/`poll`; a `submit_step` that runs the submit command, parses
  `job_handle=`, and records a Submitted attempt; a `poll_submitted_attempt` that
  runs the poll command, maps status, and on `succeeded` collects outputs +
  finalizes. Fixture submit/poll scripts (local PID) in a test.
- **Phase 3 — run-loop integration + CLI.** Poll phase in `run_flow_with`;
  submitted steps block dependents; `run` resumability; `jobs list`/`jobs poll`.
- **Phase 4 — robustness.** Reconnect after a crash (a fresh `run` re-attaches by
  polling stored handles), submitted-job timeout, and detached-job cancellation
  (poll + a `cancel` command).

## 8. Invariants / honesty / boundaries

- `argument.rs` byte-identical across all phases (execution-only).
- Detached jobs run with inherited env (like forage/nextflow tools) — egress and
  scheduler auth are the submit script's responsibility; documented, not a core
  concern.
- Caching: a detached job's collected outputs cache under the same cache key as a
  synchronous run of the same tool+inputs+params+runtime, so a cache hit still
  short-circuits before submit.
- No scheduler-specific code in the core: SLURM / Nextflow-bg / cloud all live in
  user submit/poll scripts. Phases 1–3 ship with only a local-PID fixture.
- Not in scope: distributed run coordination across multiple AgentFlow processes;
  a single project DB is still the single writer.
