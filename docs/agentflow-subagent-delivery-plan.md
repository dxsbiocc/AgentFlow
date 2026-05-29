# AgentFlow Subagent Delivery Plan

Status: Superseded by `docs/agentflow-delivery-control-plane.md` for operating model; retained for role/task reference
Owner: Project Lead / Technical Director
Last updated: 2026-05-28
Primary scope: `docs/agentflow-v0-runtime-mvp-spec.md`

Current operating model:

- Use `docs/agentflow-delivery-control-plane.md` as the active project control plane.
- Use this document as the role catalog and subagent task-shaping reference.
- Use `docs/status/*-board.md` files as visible milestone boards.

## 1. Purpose

This document defines how AgentFlow implementation work should be assigned to subagents and reviewed.

The goal is to keep the project moving like a disciplined engineering team:

1. Split work into bounded implementation slices.
2. Give each subagent clear ownership.
3. Avoid overlapping edits and merge conflicts.
4. Require a review subagent after each implementation slice.
5. Keep V0 focused on the runtime MVP.
6. Produce visible evidence that the project is progressing safely.

The project lead remains responsible for integration, scope control, and final acceptance. Subagents implement or review bounded tasks; they do not redefine the product scope.

## 2. Operating Model

AgentFlow delivery should use a leader-worker-reviewer loop.

```text
Project Lead
  -> assigns bounded implementation task
  -> Implementation Subagent edits owned files
  -> Review Subagent reviews the completed slice
  -> Project Lead integrates, fixes, or reassigns
  -> Verification runs
  -> Task marked accepted
```

Each implementation task must have:

- Clear purpose
- Owned files or modules
- Explicit non-goals
- Expected tests
- Expected CLI behavior if applicable
- Completion evidence
- Assigned review role

Each review must answer:

- Does it satisfy the task?
- Does it stay inside scope?
- Does it break module boundaries?
- Are tests meaningful?
- Are docs/schema updated if required?
- What risks remain?

## 3. Subagent Roles

### Project Lead

Owned by the main agent.

Responsibilities:

- Maintain scope and priorities.
- Assign tasks.
- Prevent overlapping work.
- Integrate completed slices.
- Decide when to stop or defer work.
- Run final verification.
- Keep product and technical docs aligned.

### Implementation Subagent

Recommended role: `executor`.

Responsibilities:

- Implement one bounded feature slice.
- Edit only assigned files/modules.
- Add or update focused tests.
- Report changed files and verification results.
- Avoid reverting unrelated changes.
- Leave TODOs only when approved by the task scope.

### Review Subagent

Recommended role: `code-reviewer` or `verifier`.

Responsibilities:

- Review the implementation slice after completion.
- Prioritize bugs, missing tests, scope creep, unsafe behavior, and compatibility issues.
- Give file/line-specific findings when possible.
- Approve, request fixes, or recommend deferral.

### Architecture Review Subagent

Recommended role: `architect`.

Used when a task changes:

- Core state model
- Runtime loop
- Storage schema
- Public CLI/API contracts
- Module boundaries

### Test Review Subagent

Recommended role: `test-engineer`.

Used when a task adds:

- Runtime behavior
- Retry/cache semantics
- State transitions
- CLI command behavior
- Failure handling

### Security Review Subagent

Recommended role: `security-reviewer`.

Used when a task touches:

- Command execution
- Environment variables
- File deletion
- Path handling
- Network access
- Secrets

## 4. V0 Workstreams

V0 should be split into disjoint workstreams.

Subagents should be assigned by **write domain and contract**, not by CLI command. Commands like `run`, `retry`, `status`, `logs`, and `report` all touch shared run/artifact/event state; splitting by command would create conflicts and inconsistent state transitions.

| Workstream | Owner Type | Primary Files/Modules | Review Role |
| --- | --- | --- | --- |
| Schema owner | executor | versioned structs/contracts, golden fixtures | architect |
| Storage owner | executor | SQLite migrations, repository layer, filesystem layout | code-reviewer + test-engineer |
| Runtime owner | executor | state machine, DAG ready-step, executor orchestration, retry/cache transitions, event emission | architect + test-engineer |
| Tooling/validation owner | executor | tool spec parser, registry, preflight/postflight, observer hook | code-reviewer |
| Artifact owner | executor | artifact import, hash, metadata, path policy | code-reviewer + security-reviewer |
| CLI owner | executor | command routing, human output, JSON output | verifier |
| Report owner | executor | markdown report from runs/artifacts/observations | code-reviewer |
| Acceptance owner | test-engineer | demo data, golden outputs, integration tests | verifier |

### Table Write Ownership

V0 should treat table writes as controlled interfaces.

| Tables | Write Owner |
| --- | --- |
| `runs`, `run_attempts`, `events`, `cache_entries` | Runtime owner |
| `tools`, `tool_versions` | Tooling/validation owner |
| `artifacts` | Artifact owner through ArtifactService; runtime may register computed outputs through the same service |
| `reports` | Report owner |
| `projects`, `flows`, `steps`, `edges` | Storage owner, with schema owner review for structural changes |

CLI code must not write SQLite directly. It should call core services.

V0 should not assign implementation owners for Omiga, Research Mode, Catalog, Hypotheses, Docker, Nextflow, or literature retrieval. These are backlog/interface-reservation topics, not V0 code paths.

## 5. Recommended Implementation Order

### Milestone 0: Repository Foundation

Goal:

Create a buildable, testable project skeleton.

Tasks:

1. Create CLI/core module layout.
2. Add formatting/lint/test commands.
3. Add minimal `agentflow --version`.
4. Add CI-ready test structure if applicable.

Review gate:

- Build succeeds.
- Test command runs.
- No runtime behavior yet.

### Milestone 1: Domain Model and Storage

Goal:

Represent projects, flows, steps, edges, tools, artifacts, runs, events, and reports.

Tasks:

1. Implement V0 domain structs.
2. Implement schema version constants.
3. Implement SQLite migrations.
4. Implement repository functions for project init/open.
5. Implement event append.

Review gate:

- Migration tests pass.
- Project init creates expected `.agentflow/` layout.
- Schema is smaller than the long-term technical design and matches V0 spec.

### Milestone 2: Tool and Artifact Registration

Goal:

Register executable local tools and imported artifacts.

Tasks:

1. Parse `agentflow.tool.v0`.
2. Validate tool inputs, outputs, params, runtime command.
3. Register tool versions immutably.
4. Import artifact by reference or copy.
5. Hash artifacts according to V0 policy.

Review gate:

- Invalid tool specs fail clearly.
- Artifact import records path/hash/type.
- No command execution occurs during registration.

### Milestone 3: Flow Validation

Goal:

Validate a simple DAG before running.

Tasks:

1. Parse flow YAML.
2. Resolve tool references.
3. Resolve input artifacts and upstream outputs.
4. Detect missing inputs.
5. Detect cycles.
6. Store approved flow.

Review gate:

- Cycle tests pass.
- Missing input tests pass.
- Flow validation produces machine-readable errors.

### Milestone 4: Runtime and Local Executor

Goal:

Run a simple local flow.

Tasks:

1. Implement ready-step scheduler.
2. Implement V0 state transitions.
3. Create immutable workdirs.
4. Materialize `command.sh`, `inputs.json`, `params.json`, `runtime.json`.
5. Execute local process.
6. Capture stdout/stderr.
7. Record attempts and events.

Review gate:

- Successful command marks step completed.
- Nonzero exit marks step failed.
- Workdir and logs are inspectable.
- Path handling is reviewed for safety.

### Milestone 5: Validation, Cache, and Retry

Goal:

Make reruns and failures understandable.

Tasks:

1. Add preflight validation.
2. Add postflight validation.
3. Compute cache key.
4. Implement cache hit/miss explanation.
5. Implement retry without deleting prior attempts.

Review gate:

- Cache hit test passes.
- Missing output fails postflight.
- Retry creates a new attempt.
- Cache metadata is explainable.

### Milestone 6: Report and Acceptance Demo

Goal:

Produce a complete V0 demo.

Tasks:

1. Add basic observer summaries.
2. Generate markdown report.
3. Add demo marker-discovery fixtures.
4. Add end-to-end test.
5. Add `status --json`.

Review gate:

- Demo runs from imported artifacts to report.
- Report includes inputs, steps, outputs, observations, failures if any.
- `status --json` is stable and parseable.

## 6. Review Workflow

Every implementation subagent completion triggers a review subagent.

Recommended pattern:

```text
assign implementation task
  -> implementation subagent completes
  -> automatic gate runs
  -> if automatic gate fails, return to implementation
  -> project lead reads changed files and summary
  -> spawn review subagent with:
       task brief
       changed files
       acceptance criteria
       test results
  -> review subagent returns findings
  -> project lead decides:
       accept
       request fixes
       defer issue
       split follow-up task
```

Review subagent should not make large feature changes. It should review first. If fixes are small, the project lead may apply them or assign a follow-up implementation task.

Automatic gate must pass before review subagent review begins.

Automatic gate includes:

- formatting
- linting, if configured
- relevant unit tests
- focused integration tests
- schema/golden validation when outputs changed
- migration validation when storage changed
- failure-injection test when runtime/cache/retry changed

If the automatic gate fails, the task returns to the implementation subagent with the failing evidence. The review subagent is only used after the implementation is mechanically reviewable.

## 7. Review Severity

Findings should use severity:

| Severity | Meaning | Required Action |
| --- | --- | --- |
| P0 | Data loss, unsafe execution, wrong state corruption | Block merge |
| P1 | Core behavior wrong, tests missing for critical path | Block merge |
| P2 | Important quality issue, manageable workaround | Fix or explicitly defer |
| P3 | Minor cleanup or documentation improvement | Optional |

Default policy:

- P0/P1 must be fixed before acceptance.
- P2 may be deferred only with written rationale.
- P3 should not block delivery.

## 8. Task Handoff Template

Each implementation task should use this template.

```text
Task:
  <short task name>

Goal:
  <what user-visible or runtime behavior this enables>

Owned files/modules:
  <paths or modules>

Inputs:
  <relevant docs, schemas, prior tasks>

Required behavior:
  <specific bullets>

Non-goals:
  <explicit exclusions>

Tests required:
  <unit/integration/golden/manual>

Completion evidence:
  <commands run, outputs, changed files>

Review subagent:
  <code-reviewer | verifier | architect | test-engineer | security-reviewer>
```

## 9. Review Handoff Template

Each review task should use this template.

```text
Review task:
  <implementation slice name>

Context:
  <why this slice exists>

Changed files:
  <paths>

Acceptance criteria:
  <expected behavior>

Verification evidence:
  <tests/commands run by implementation subagent>

Review focus:
  - correctness
  - scope control
  - module boundaries
  - test adequacy
  - safety/security if relevant

Output format:
  Findings first, ordered by severity.
  Include file/line references where possible.
  Include residual risks and recommended next action.
```

## 10. Project Control Rules

1. No subagent may expand V0 scope without project lead approval.
2. No subagent may introduce Omiga dependency into core runtime.
3. No subagent may add catalog/capability search, Research Mode, hypotheses, Omiga adapter, Docker, Singularity, Nextflow, remote execution, parallel scheduler, automatic dependency installation, or literature retrieval to V0 implementation.
4. No subagent may edit another subagent's owned files without coordination.
5. No implementation slice is accepted without review.
6. No task is done without test or explicit reason why no test applies.
7. No destructive file operation is allowed by default.
8. No generated report may claim biological truth beyond observed fixture/demo evidence.
9. No subagent may bypass core services and write internal SQLite tables directly from CLI code.
10. No subagent may treat the long-term technical design as overriding the narrower V0 MVP spec.

## 11. Delivery Dashboard

The project lead should maintain a simple status table.

| Milestone | Status | Owner | Reviewer | Evidence | Risk |
| --- | --- | --- | --- | --- | --- |
| M0 Foundation | pending | TBD | TBD | TBD | TBD |
| M1 Domain/Storage | pending | TBD | TBD | TBD | TBD |
| M2 Tool/Artifact | pending | TBD | TBD | TBD | TBD |
| M3 Flow Validation | pending | TBD | TBD | TBD | TBD |
| M4 Runtime/Executor | pending | TBD | TBD | TBD | TBD |
| M5 Cache/Retry | pending | TBD | TBD | TBD | TBD |
| M6 Report/Demo | pending | TBD | TBD | TBD | TBD |

## 12. Automatic Review Policy

The automation policy is:

1. Implementation subagent finishes.
2. Automatic gate runs.
3. If automatic gate fails, the implementation subagent fixes the issue before review.
4. Project lead collects changed file list and test evidence.
5. Project lead immediately spawns a review subagent.
6. Review subagent reports findings.
7. If blocking findings exist, project lead assigns a fix task.
8. If no blocking findings exist, task is accepted.

In the current Codex environment, the project lead triggers the review subagent as the orchestration action. In a future implementation, AgentFlow can encode this as a workflow rule:

```text
on_task_completed:
  if task.type == implementation:
    create_review_task(task.changed_files, task.acceptance_criteria)
```

This keeps the review behavior explicit today and automatable later.

## 13. Blocker and Warning Policy

Review findings should be classified as blocker or warning.

### Blockers

These block acceptance:

- Main V0 scenario cannot run.
- Illegal state transition.
- Cache hit can reuse invalid or missing outputs.
- Retry overwrites previous attempts.
- Failure lacks actionable diagnostics.
- CLI JSON output is not parseable or is unstable without versioning.
- Logs or large artifacts are stored as database blobs.
- CLI writes internal SQLite tables directly.
- Omiga, Research Mode, Catalog, Docker, Nextflow, or literature retrieval enters V0 runtime path.
- Generated report claims biological truth beyond demo evidence.

### Warnings

These may be accepted with written risk and backlog item:

- Golden test coverage is partial but main path is tested.
- Large-file hash policy remains conservative.
- `doctor` is basic.
- Report wording is plain but accurate.
- Observer support only covers the first demo tool.
- JSON output contains additive fields that are versioned and documented.

## 14. Strong Gates

The project lead should enforce four strong gates:

1. **Runtime Skeleton Gate**: project init, schema, tool registration, flow validation, two-step local run, workdir/logs/status.
2. **Validation and Cache Gate**: preflight, postflight, cache explain, retry, missing-output failure.
3. **Report and JSON Gate**: markdown report, `status --json`, artifact visibility, no Omiga dependency.
4. **End-to-End V0 Gate**: marker-discovery demo from imported artifacts through failure/retry/cache/report.

Other gates may be lighter, but these four determine whether the project is actually deliverable.
