# AgentFlow Delivery Control Plane

Status: Active operating model
Owner: Project Lead / Technical Director
Last updated: 2026-05-28

## 1. Why This Document Exists

The first M0-M4 slices deliberately moved in small increments to establish a safe foundation:

- project-local SQLite storage
- tool registry
- artifact registry
- static flow validation
- versioned CLI JSON contracts

That was useful for de-risking the base, but it should not remain the permanent delivery style.

If every next step is a tiny command or table update, AgentFlow can become locally consistent but strategically wrong. The project needs a control plane that keeps implementation aligned with the product thesis:

> AgentFlow is a CLI-first scientific workflow runtime that can execute, explain, retry, cache, and report a real local analysis, while remaining independent and Omiga-ready.

## 2. Delivery Philosophy

AgentFlow should now move from micro-slices to milestone bundles.

Micro-slice delivery is allowed only when:

- a module boundary is being created for the first time
- schema compatibility risk is high
- a dependency decision is unresolved
- a failing test must be isolated

Normal delivery should use milestone bundles:

```text
macro goal
  -> acceptance scenario
  -> workstream tasks
  -> parallel implementation agents
  -> reviewer agents
  -> integration gate
  -> demo evidence
```

The project lead must protect the macro goal. Subagents can implement or review slices, but they do not redefine the target.

## 3. Control Plane Layers

### 3.1 Product Control

Product control answers:

- Does this move AgentFlow toward a runnable scientific workflow?
- Does it preserve independent CLI-first value?
- Does it avoid premature Omiga, Research Mode, Nextflow, container, or UI coupling?
- Does it support intermediate-start workflows such as BAM, H5AD, tables, or reports?

Every milestone must declare:

- user-visible outcome
- acceptance demo
- non-goals
- risks that would make the milestone misleading

### 3.2 Architecture Control

Architecture control answers:

- Are core/CLI/schema boundaries still clean?
- Is SQLite accessed only through core services?
- Are JSON contracts versioned?
- Is runtime correctness independent of UI state?
- Are deterministic runtime and Agent reasoning still separated?

Any milestone touching runtime, schema, command execution, or state transitions requires architect review.

### 3.3 Execution Control

Execution control answers:

- Which workstreams can run in parallel?
- Which table/module owns each state transition?
- Which tests prove the claim?
- What must be integrated before the milestone is accepted?

Implementation agents must receive:

- owned files/modules
- explicit write boundaries
- non-goals
- expected tests
- expected CLI behavior
- handoff format

### 3.4 Review Control

Review control answers:

- Did the implementation satisfy the assigned task?
- Did it introduce scope creep?
- Did it break upgrade compatibility?
- Are tests meaningful or only superficial?
- Are failure cases diagnosable?

Every implementation bundle should have at least one review pass:

- code-reviewer for correctness and maintainability
- architect for boundaries and state model
- test-engineer for runtime/cache/retry behavior
- security-reviewer for command execution and filesystem safety
- verifier for final claim validation

If a subagent review times out, the project lead must record that explicitly and perform a manual fallback review. Silent review failure is not allowed.

## 4. Current Project State

Implemented:

- M0 repository foundation
- M1 storage and migrations
- M2 tool registry
- M3 artifact import
- M4 static flow validation and approval

Current system can:

```text
init project
register tool
import artifact
validate flow
approve flow
inspect flow
```

Current system cannot yet:

```text
run a step
create workdirs
capture logs
materialize commands
validate outputs
register computed artifacts
retry failed steps
explain cache hits
generate reports
```

Therefore the next work must not be another tiny registry feature. It should be the first runtime milestone bundle.

## 5. Macro Roadmap From Here

### Bundle A: Runtime Execution Core

Goal:

> Run an approved one-step local flow and record attempts, workdir, logs, status, and computed artifacts.

Included:

- ready-step selection
- step status transition from `draft` to `ready` to `running` to terminal state
- run and run_attempt records
- immutable workdir creation
- `command.sh`, `inputs.json`, `params.json`, `runtime.json`
- stdout/stderr capture
- local process execution
- output existence validation
- computed artifact registration
- `agentflow run <flow-id>`
- `agentflow logs <run-or-attempt-id>`

Non-goals:

- cache
- retry
- parallel scheduling
- Conda/Docker/Singularity
- Agent failure explanation

Required reviewers:

- architect
- code-reviewer
- security-reviewer
- test-engineer

Acceptance:

```text
agentflow init
agentflow tools register examples/tools/...
agentflow import examples/data/expression.tsv --type TSV
agentflow flow approve examples/flows/...
agentflow run marker_demo
agentflow status --json
agentflow logs <attempt-id>
agentflow artifacts list --json
```

### Bundle B: Retry, Cache Metadata, and Diagnostics

Goal:

> Failed steps can be retried without destroying previous attempts, and repeated successful steps can explain cache behavior.

Included:

- `agentflow retry <step-id>`
- run attempt history
- cache key computation
- cache_entries writes
- cache miss/hit explanation
- status summaries for failed/blocked/completed steps
- clearer validation failure messages

Non-goals:

- distributed cache
- content-addressed artifact store
- cloud cache

Required reviewers:

- architect
- test-engineer
- verifier

### Bundle C: Report and Scientific Demo

Goal:

> Complete a small tumor marker demo from imported tables to final markdown report.

Included:

- fixture data
- one or two local tools that are executable in CI
- basic observer summaries
- report generation from artifacts, runs, logs, observations, and failures
- negative/inconclusive result wording

Non-goals:

- biological publication claims
- automated literature support
- Research Mode

Required reviewers:

- code-reviewer
- verifier
- product/critic review

### Bundle D: Hardening and API Hygiene

Goal:

> Reduce foundation debt before adding more intelligence.

Included:

- evaluate adding `serde`, `serde_json`, and `serde_yaml`
- replace hand-rendered JSON where justified
- formalize public JSON schemas
- improve parser error messages
- create integration test fixtures
- update docs into one coherent V0 guide

Non-goals:

- broad feature expansion

Required reviewers:

- architect
- code-reviewer
- test-engineer

## 6. Subagent Execution Model

For milestone bundles, use a leader-supervised multi-agent loop.

```text
1. Project Lead writes bundle brief.
2. Architect subagent reviews architecture risk.
3. Implementation subagents work on disjoint modules.
4. Project Lead integrates.
5. Review subagents inspect completed code.
6. Verifier subagent checks acceptance evidence.
7. Project Lead records outcome and residual risk.
```

### Parallelism Rules

Run in parallel only when write scopes are disjoint.

Good parallel split for Bundle A:

| Lane | Owner | Files |
| --- | --- | --- |
| Runtime state | executor | `crates/agentflow-core/src/runtime/*`, storage run APIs |
| Local executor | executor | `crates/agentflow-core/src/executor/*` |
| CLI runtime commands | executor | `crates/agentflow-cli/src/lib.rs` |
| Integration tests | test-engineer | test fixtures and CLI tests |

Do not split by user command if the commands share the same state transition logic.

### Review Automation Rules

After each implementation lane:

1. spawn a reviewer with the lane scope
2. reviewer must produce findings first
3. project lead fixes or defers findings
4. final verifier checks the complete bundle

If reviewer does not return:

- record timeout in `docs/status/<bundle>-review.md`
- perform direct manual review
- do not pretend the subagent approved

## 7. Monitoring Board

Each macro bundle should have a status board under `docs/status/`.

Required fields:

```text
Bundle:
Goal:
State: planned | in_progress | review | accepted | blocked
Owner:
Implementation lanes:
Review lanes:
Acceptance demo:
Automatic gates:
Manual gates:
Blocking risks:
Residual risks:
```

The project lead updates the board at:

- bundle start
- lane assignment
- implementation completion
- review completion
- verification completion
- acceptance or block

This is the project-level “heartbeat.” Without it, subagent work becomes invisible and easy to misinterpret.

## 8. Kill Criteria

A milestone bundle must stop or be redesigned if:

- it requires introducing a deferred subsystem to pass acceptance
- it weakens CLI-first independence
- it depends on Omiga UI state
- it executes arbitrary Agent-generated shell without registered tool boundaries
- it cannot produce a reproducible local demo
- review finds state-model or safety defects that cannot be fixed locally

Kill criteria are not failure. They prevent false progress.

## 9. Immediate Change To Operating Mode

Effective now:

- Stop treating each CLI command as a milestone.
- Treat Runtime Execution Core as one bundle.
- Use subagents for architecture review, implementation lanes, code review, and verification where available.
- Maintain a visible bundle board.
- Record subagent failures honestly.
- Do not advance to cache/report/Research Mode until a local runtime demo works.
