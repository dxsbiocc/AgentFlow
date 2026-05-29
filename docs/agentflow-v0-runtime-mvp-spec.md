# AgentFlow V0 Runtime MVP Specification

Status: Draft for review
Owner: TBD
Last updated: 2026-05-28
Related documents:

- `docs/agentflow-product-development.md`
- `docs/agentflow-technical-design.md`

## 1. Purpose

This document narrows AgentFlow into the smallest runnable product slice.

The long-term technical design describes the full direction: runtime, evidence graph, Agent graph patches, Research Mode, tool discovery, hypothesis critique, reports, and future Omiga integration.

V0 should not attempt all of that.

V0 should prove one thing:

> AgentFlow can manage a real local scientific task graph from imported artifacts to executable steps, logs, status, retry, cache metadata, validated outputs, and a basic report.

If V0 cannot run and explain a small flow reliably, advanced Agent reasoning and Research Mode will not matter.

## 2. V0 Product Contract

V0 is a CLI-first local runtime.

It should support:

1. Initialize an AgentFlow project.
2. Register a small set of local tools.
3. Import existing artifacts.
4. Validate a simple flow.
5. Execute a DAG with local process backend.
6. Create isolated work directories.
7. Capture stdout/stderr.
8. Track run status and attempts.
9. Retry failed steps.
10. Register output artifacts.
11. Compute cache keys and explain cache hits/misses.
12. Generate a basic markdown report from run metadata and observations.

V0 should not require:

- Omiga
- Tauri
- React
- Docker
- Singularity
- Nextflow
- Automated web search
- Automated literature retrieval
- Multi-user collaboration
- Full Agent autonomy

## 3. V0 Non-Goals

V0 explicitly does not include:

- Full Research Mode
- Literature full-text retrieval
- GitHub/package repository search
- Automated tool installation
- Tool marketplace
- Docker/Singularity execution
- Remote/HPC/cloud execution
- Complex scatter/gather
- Dynamic channel semantics
- Visual workflow editor
- Omiga UI integration
- Publishing-quality biological conclusions

Research notes, hypotheses, catalog entries, and citations can be modeled later. V0 may reserve JSON fields for future provenance, but should not build the complete subsystem yet.

## 4. First Domain Decision

The first domain must be narrow enough to validate AgentFlow without becoming a bioinformatics platform.

Recommended first domain:

> Tumor marker discovery from existing expression and clinical/survival tables.

Why this is a good V0 domain:

- Starts naturally from intermediate artifacts.
- Avoids large raw sequencing pipelines in the first demo.
- Exercises imported artifact roots.
- Produces meaningful positive, negative, and inconclusive results.
- Supports report generation.
- Supports later hypothesis branching to homologs, interactors, pathways, and cohorts.
- Can be implemented with small tabular fixture data.

Alternative domains:

| Domain | Strength | Risk |
| --- | --- | --- |
| Fusion analysis | Good negative-result story | Tool/runtime complexity is higher |
| scRNA | Strong visualization value | Data and environment burden are heavier |
| bulk RNA-seq | Familiar workflows | Can pull V0 toward standard pipeline rebuilding |
| General omics | Broad appeal | Too vague for V0 acceptance |

V0 should choose one first scenario and defer broad domain support.

## 5. V0 Minimal CLI

The V0 CLI should stay small.

Required commands:

```text
agentflow init
agentflow doctor
agentflow tools register <tool.yaml>
agentflow tools list
agentflow tools inspect <tool-ref>
agentflow import <path> --type <artifact-type>
agentflow flow validate <flow.yaml>
agentflow flow approve <flow.yaml>
agentflow run <flow-id-or-yaml>
agentflow run-step <step-id>
agentflow status
agentflow status --json
agentflow logs <run-id>
agentflow retry <step-id>
agentflow artifacts list
agentflow artifacts inspect <artifact-id>
agentflow cache explain <step-id>
agentflow report generate <flow-id>
```

Deferred commands:

```text
agentflow catalog ...
agentflow research ...
agentflow hypotheses ...
agentflow env prepare ...
agentflow tools promote ...
agentflow graph export ...
agentflow cleanup ...
agentflow backup ...
```

Rationale:

- `status --json` is enough for early automation and future Omiga probing.
- Catalog, research, and hypothesis commands need deeper product rules and should not block runtime MVP.
- Cleanup and backup are important, but can follow once the project layout stabilizes.

## 6. V0 Minimal Database Schema

V0 should use the smallest schema that supports execution, status, artifacts, and basic reports.

Required tables:

```text
schema_migrations
projects
flows
steps
edges
tools
tool_versions
artifacts
runs
run_attempts
cache_entries
observations
events
reports
```

Deferred tables:

```text
tool_catalog_entries
tool_capabilities
hypotheses
research_notes
research_sources
research_queries
research_hits
research_documents
citations
evidence_claims
approvals
```

The deferred concepts can be represented temporarily as structured JSON payloads in `events`, `observations`, or `reports` if needed.

### Required V0 Fields

`projects`

- `id`
- `name`
- `root_path`
- `created_at`
- `updated_at`
- `engine_version`

`flows`

- `id`
- `name`
- `status`
- `source_path`
- `schema_version`
- `created_at`
- `updated_at`

`steps`

- `id`
- `flow_id`
- `tool_ref`
- `type`
- `status`
- `reason`
- `params_json`
- `inputs_json`
- `outputs_json`
- `created_at`
- `updated_at`

`edges`

- `id`
- `flow_id`
- `from_step_id`
- `to_step_id`
- `edge_type`

`tools`

- `id`
- `name`
- `namespace`
- `latest_version`
- `maturity`

`tool_versions`

- `id`
- `tool_id`
- `version`
- `schema_version`
- `spec_json`
- `spec_hash`
- `created_at`

`artifacts`

- `id`
- `kind`
- `type`
- `path`
- `hash`
- `size_bytes`
- `source_step_id`
- `source_run_id`
- `validation_json`
- `created_at`

`runs`

- `id`
- `flow_id`
- `step_id`
- `status`
- `attempt_count`
- `latest_attempt_id`
- `cache_key`
- `created_at`
- `updated_at`

`run_attempts`

- `id`
- `run_id`
- `attempt`
- `status`
- `workdir`
- `started_at`
- `ended_at`
- `exit_code`
- `stdout_path`
- `stderr_path`
- `error_class`
- `error_message`

`cache_entries`

- `cache_key`
- `tool_ref`
- `input_hashes_json`
- `params_hash`
- `runtime_hash`
- `output_artifacts_json`
- `created_at`
- `last_used_at`

`observations`

- `id`
- `flow_id`
- `step_id`
- `artifact_id`
- `kind`
- `severity`
- `summary`
- `payload_json`
- `created_at`

`events`

- `id`
- `flow_id`
- `step_id`
- `run_id`
- `event_type`
- `payload_json`
- `created_at`

`reports`

- `id`
- `flow_id`
- `format`
- `path`
- `created_at`

## 7. V0 Run State Machine

V0 should implement a small but strict state machine.

### Step States

```text
draft
waiting_for_input
ready
running
completed
completed_with_warning
failed
skipped
superseded
```

Deferred states:

```text
waiting_for_approval
stopped_negative
cancelled
```

These deferred states matter later, but V0 can model negative results as observations and cancellation as failed attempt with `error_class = cancelled` until the state machine matures.

### Legal Transitions

```text
draft -> waiting_for_input
draft -> ready
waiting_for_input -> ready
ready -> running
running -> completed
running -> completed_with_warning
running -> failed
failed -> ready
ready -> skipped
completed -> superseded
completed_with_warning -> superseded
```

Rules:

1. A step becomes `ready` only when all required inputs resolve to valid artifacts or upstream outputs.
2. A `running` step must have one active run attempt.
3. A `completed` step must have validated outputs.
4. A `failed` step may be retried by creating a new run attempt.
5. Retrying does not delete prior attempts.
6. Superseding never deletes old artifacts or runs.

### Run Attempt States

```text
created
running
succeeded
failed
timed_out
cancelled
cache_hit
```

Rules:

1. Every execution attempt gets its own immutable workdir.
2. `cache_hit` attempts do not run a command but must validate restored outputs.
3. Timeout is a failure class, not a successful negative result.
4. The latest attempt determines current run status, but previous attempts remain inspectable.

## 8. V0 Runtime Behavior

### Execution Loop

```text
load flow
validate graph
resolve ready steps
for each ready step:
  validate inputs
  compute cache key
  if cache hit and outputs valid:
    record cache_hit attempt
    register outputs
    mark completed
    continue
  create workdir
  materialize command.sh
  write inputs.json, params.json, runtime.json
  run local process
  capture stdout.log and stderr.log
  validate outputs
  register artifacts
  create observations
  update status
repeat until no ready steps remain
```

### V0 Scheduler Rules

- Default concurrency is `1`.
- Parallel execution is deferred.
- No background daemon is required.
- A single `agentflow run` command should run until no runnable steps remain.
- If a step fails, downstream steps remain blocked.
- Exit code should be nonzero if the flow has failed steps.

## 9. V0 Artifact Contract

Artifacts are first-class project objects.

### Artifact Kinds

```text
imported
computed
report
log
summary
```

### Import Modes

V0 should support:

- `reference`: keep artifact at original path and store hash/metadata.
- `copy`: copy artifact into `.agentflow/artifacts/imported/`.

Default:

- Small files can be copied.
- Large files should be referenced by default.

### Required Metadata

Every artifact should record:

- logical type
- path
- hash when feasible
- size
- source step/run if computed
- import mode if imported
- validation result

### Hash Policy

V0 can use SHA-256 for small and medium files.

For very large files, V0 may record:

- size
- mtime
- optional full hash
- optional partial hash
- user-visible hash policy

The cache system should mark weak hashes as lower confidence.

## 10. V0 Tool Contract

V0 tools should be local and explicit.

Required tool spec fields:

```yaml
schema_version: agentflow.tool.v0
name: string
version: string
maturity: verified | wrapped | exploratory
description: string
inputs: map
params: map
outputs: map
runtime:
  backend: local
  command: list
validators:
  preflight: list
  postflight: list
observer:
  name: string
```

V0 should support:

- local process backend
- command templates
- declared inputs
- declared outputs
- parameter schema
- basic validators
- basic observer output

V0 should not support:

- automatic environment solving
- Docker/Singularity
- automatic package install
- arbitrary Agent-generated shell as verified tool

## 11. V0 Validation Contract

### Preflight Checks

Required:

- Input artifact exists.
- Input artifact type matches tool input type.
- Required params exist.
- Param values match schema.
- Output paths are inside workdir or declared artifact output directory.
- Tool runtime backend is available.

### Postflight Checks

Required:

- Command exit code is acceptable.
- Declared output files exist.
- Declared output files are readable.
- Output files are non-empty unless the tool spec allows empty outputs.
- Artifact hashes are recorded.
- Basic observer summary is created if observer is configured.

## 12. V0 Agent Safety Contract

V0 should keep Agent behavior narrow.

Allowed:

- Generate a draft flow from a user goal and registered tools.
- Explain a failure using logs and validation results.
- Suggest a graph patch as text/JSON for user review.
- Generate report prose from existing observations.

Not allowed:

- Execute commands directly.
- Install dependencies.
- Modify tool specs silently.
- Register new tools without user command.
- Delete artifacts.
- Claim literature-backed biological conclusions without citations.

V0 can run without any Agent calls. The deterministic runtime must stand on its own.

## 13. V0 Omiga Integration Contract

V0 should be independent but integration-ready.

Omiga should not rely on internal SQLite tables as its public contract.

Allowed future integration surfaces:

- `agentflow status --json`
- `agentflow artifacts list --json`
- `agentflow report generate ... --format json`
- exported flow/graph JSON
- stable artifact/report manifest files
- later `agentflow-core` library API

Not allowed:

- Omiga reading arbitrary internal SQLite tables as a stable API.
- AgentFlow core importing Omiga modules.
- Runtime state living only inside Omiga.
- UI state controlling runtime correctness.

V0 should include enough JSON output to make future Omiga integration predictable, but should not build the integration yet.

## 14. V0 Acceptance Test

V0 must pass one end-to-end test.

Recommended scenario:

> Evaluate whether a gene is a tumor marker using imported expression and survival tables.

### Test Steps

1. `agentflow init`
2. Register `marker_survival_scan` tool.
3. Register `related_gene_scan` or a small second analysis tool.
4. Import expression table.
5. Import survival table.
6. Validate flow YAML.
7. Run flow.
8. Inspect status.
9. Inspect logs.
10. Inspect artifacts.
11. Confirm cache miss on first run.
12. Re-run flow.
13. Confirm cache hit or skipped completed step.
14. Force one failure with missing output.
15. Confirm failed status and readable error.
16. Retry after fixing configuration.
17. Generate markdown report.

### Acceptance Criteria

V0 is acceptable only if:

- A user can run the scenario from CLI without Omiga.
- The flow can start from imported artifacts.
- Each step gets a workdir.
- Logs are readable.
- Failure is diagnosable.
- Retry does not destroy previous attempts.
- Cache behavior is explainable.
- Outputs are registered as artifacts.
- Report includes inputs, steps, outputs, observations, and failures if any.
- `status --json` is machine-readable and stable enough for future consumers.

## 15. V0 Success Metrics

Product success:

- A first-time technical user can complete the demo in under 30 minutes after dependencies are ready.
- The final report explains what ran, why it ran, and where outputs are.
- A failed step can be diagnosed without reading source code.
- Rerun avoids unnecessary execution when cache is valid.

Engineering success:

- Unit tests cover tool spec validation, graph validation, state transitions, and cache key generation.
- Integration tests cover success, failure, retry, and cache hit.
- Fixture data is small enough to run in CI.
- No Omiga dependency exists in core runtime.
- No frontend framework is required.

## 16. Deferred Backlog

After V0:

1. Conda/micromamba backend.
2. Stronger observers.
3. Graph patch approval.
4. Hypothesis records.
5. Discovery catalog.
6. Research Mode.
7. Literature source adapters.
8. User PDF import.
9. Omiga adapter.
10. Docker/Singularity.
11. Nextflow/Snakemake import/export.

This backlog should not block the first runnable runtime.
