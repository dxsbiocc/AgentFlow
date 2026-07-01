# Changelog

All notable changes to AgentFlow are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). This is a pre-1.0
technical preview; the public API and CLI surface may change between minor versions.

## [Unreleased]

### Added

- **Async/detached execution — `submit_step` (phase 2b).** Runs a detached tool's
  submit command, parses its `job_handle=`, and marks the run attempt `Submitted`
  (running detached) — reusing `prepare_step` (whose built command is the submit
  argv for a detached tool). A submit that fails or prints no handle falls through
  to the normal record path (recorded failed, since a submit produces no declared
  outputs). Poll/collect and run-loop integration follow.

### Added

- **Async/detached execution — the `detached` tool contract (phase 2a).** A tool
  can declare `runtime.backend: detached` with a submit command (`runtime.command`,
  which prints `job_handle=<id>`) and a new `runtime.poll` command (which prints
  `status=running|succeeded|failed`). Registration validates the contract (both
  commands absolute; no env/image/runner; `poll` only on detached tools). Adds the
  `DetachedBackend` (submit argv construction) and the `job_handle=` / `status=`
  parse helpers. The `poll` field is cache-key-stable (`skip_serializing_if`) so
  existing tools' cache keys are byte-identical. Submit/poll execution and run-loop
  integration follow in later phases.

### Added

- **Async/detached execution — persistence substrate (design + phase 1).** Groundwork
  for submit→poll→collect execution of long-running / HPC / Nextflow jobs: a new
  `RunAttemptStatus::Submitted`, a `run_attempts.job_handle` column (migration v3),
  and `ProjectStore` methods to record a submitted attempt, list outstanding
  submitted attempts per flow, and finalize one to a terminal status. Storage-only —
  no run-loop or backend change yet. See `docs/design/async-execution-design.md`.

## [0.4.1] - 2026-06-30

Completes the first-class module line: a module answering a hypothesis fills its
inferable params (e.g. the gene) from the hypothesis. The deterministic 0-LLM
verdict core (`argument.rs`) remains byte-identical to 0.3.0.

### Added

- **Per-hypothesis param inference for module answers (slice 4b-4c).** When the
  agent answers a hypothesis with a registered module, an inferable param
  (`infer:` hint, e.g. `gene`) that the module leaves unset is now filled from the
  hypothesis (e.g. the gene symbol), instead of requiring a fixed module value.
  The inferred value is recorded as an unconfirmed inferred param, so the verdict
  stays grade-capped (honesty interlock) — exactly as for a tool answer.
  Module-set params are never overwritten.

## [0.4.0] - 2026-06-30

The autonomous agent can now discover and compose registered **modules**
(reusable typed sub-flows) on its own — both as intermediate producers and as the
top-level answer to a hypothesis — and modules persist in the project. The
deterministic 0-LLM verdict core (`argument.rs`) remains byte-identical to 0.3.0,
and the scheduler/runtime are unchanged.

### Added

- **The agent answers with a registered module (slice 4b-3b).** When no tool can
  answer a hypothesis, the autonomous loop now falls back to a registered module
  that produces an observation (an internal step whose tool has an observed
  output): the module is inline-expanded, its observed step becomes the answer and
  the rest become prerequisites. It also falls back to a module when a tool
  "answers" but its drafted step has unresolved required inputs/params. Module
  answer steps use the module's fixed params (no per-hypothesis inference yet).

### Added

- **Module answer-capability discovery (`answer_capable_modules`, slice 4b-3a).**
  `ProjectStore::answer_capable_modules` ranks registered modules that can answer
  a hypothesis — i.e. whose first internal step is a tool with an observed output
  port — High-fit (all inputs available) first, reporting the answer step + its
  observer port. Pure discovery primitive (mirrors `match_modules`); not yet wired
  into the agent's answer-matching (slice 4b-3b).

### Added

- **The agent composes registered modules (slice 4b-2).** When the autonomous
  loop backward-chains to satisfy a step's missing input type and no tool can
  produce it, it now falls back to a registered **module** whose output port
  yields that type and whose inputs are all already available (a High-fit atomic
  producer): the module is inline-expanded into the flow and wired to the
  consumer. Tools are still tried first; default behavior is unchanged when no
  module applies.

### Added

- **Module discovery for the agent (`match_modules`).** `ProjectStore::match_modules`
  ranks registered modules that can produce a desired artifact type (atomic
  `High`-fit producers — every input already available — first), the discovery
  primitive the autonomous loop will use to compose modules. Pure, not yet wired
  into the agent loop; see `docs/design/agent-module-composition-design.md` for
  the phased plan (slice 4b).

### Added

- **Persistent modules — `module register <file>` / `module list`.** Modules can
  now be registered into a project's database (migration v2 adds a `modules`
  table) and listed, mirroring the tool registry: `register_module` /
  `list_modules` / `get_module` on the store, and the two CLI commands (`list`
  supports `--json` with a `schema_version` envelope). This is the foundation for
  the agent to discover and compose modules.

## [0.3.9] - 2026-06-29

Introduces first-class **modules** — reusable, typed sub-flows composed into flows
by inline expansion — end to end (author, validate, reference, expand), plus a
retry backoff. The deterministic 0-LLM verdict core (`argument.rs`) remains
byte-identical to 0.3.0 and the scheduler/runtime are unchanged.

### Added

- **`flow validate` / `flow approve` accept `--module <file>` (repeatable).** Supply
  the module specs a flow references so its `module: <ref>` steps are inline-expanded
  when the flow is parsed; duplicate module refs are rejected. No `--module` is the
  unchanged behavior (a flow that references a module without supplying it still
  errors).

### Added

- **Flows can compose modules (`module: <ref>` step).** A flow step may reference
  a module instead of a tool; it is inline-expanded at parse time
  (`FlowDraft::from_simple_yaml_with_modules`) into ordinary tool steps —
  namespacing the module's internal steps per instance, binding its input ports
  to the step's `inputs`, exposing its outputs as `instance.port` references, and
  rewiring cross-instance `needs`. The flattened flow runs on the existing
  scheduler unchanged. (CLI wiring to load modules for `flow create` is a
  follow-up.)

### Added

- **`run --retry-backoff <seconds>` / `agent run --retry-backoff <seconds>`.** An
  optional delay before a failed-but-retried step is re-offered (pairs with
  `--retries`). `RunConfig.retry_backoff` defaults to zero (immediate retry,
  unchanged); the sleep only fires when the run is actually going to retry.
- **`module validate <file>` / `module show <file>` CLI.** Author and inspect
  `agentflow.module.v0` module specs from the command line: `validate` parses and
  validates a module YAML (reporting the ref, version, and port/step counts, or
  the validation error), and `show` pretty-prints its input/output ports and
  steps. Built on the merged `ModuleSpec` primitive; no storage yet.
- **First-class modules — inline-expansion engine (foundation).** A `ModuleSpec`
  (`agentflow.module.v0`) is a reusable, typed sub-flow: declared external
  input/output ports plus internal steps. `ModuleSpec::expand` inlines a module
  instance into ordinary flow steps — namespacing internal ids/artifacts per
  instance, rewiring external input ports to the caller's bound artifacts, and
  exposing output ports — so the existing scheduler runs the flattened DAG with
  no changes. Validation rejects dangling refs, duplicate producers, port/artifact
  name collisions, missing `needs` on internal producers, and dependency cycles.
  Library primitive only; storage, CLI, and agent composition follow in later
  slices (see `docs/status/module-expansion-proof.md`).

- **`nextflow` tool-execution backend.** A Nextflow module can be registered as
  an ordinary AgentFlow tool (`runtime.backend: nextflow`, absolute `runner` =
  the `nextflow` launcher, `command[0]` = the absolute `.nf` module path). It
  wraps `nextflow run <module> [args]` and reuses the existing
  `AGENTFLOW_INPUT_*/PARAM_*/OUTPUT_*` env convention, so the agent can run a
  module-tool on its own or backward-chain several of them with zero agent or
  scheduler changes. Registration requires the runner and module paths to be
  absolute. Runs are synchronous and egress is the user's responsibility (see
  `docs/status/nextflow-backend-proof.md`).

## [0.3.8] - 2026-06-28

Adds an auto-retry budget for transient step failures, on both the manual and
autonomous run paths. The deterministic 0-LLM verdict core (`argument.rs`)
remains byte-identical to 0.3.0, and the default behavior is unchanged.

### Added

- **`run --retries N`:** auto-retry transient step failures. A failed step is
  re-run up to `N` times (so up to `N + 1` attempts) before its failure becomes
  terminal; once it succeeds the run proceeds normally, and only an exhausted
  budget counts as a failure and trips fail-fast. Retries are immediate (no
  backoff), bounded, and apply on both the serial and parallel paths. Default
  `0` keeps the original behavior. Intended for flaky tools (network calls,
  external scripts).
- **`agent run --retries N`:** the same retry budget is now available on the
  autonomous run path, joining the existing global `--max-parallel` /
  `--keep-going` flags. It is threaded into the `RunConfig` the agent uses to
  auto-run the flows it builds. Default `0` keeps the original behavior.

## [0.3.7] - 2026-06-27

Adds skipped-step visibility to flow runs. The deterministic 0-LLM verdict core
(`argument.rs`) remains byte-identical to 0.3.0, and run semantics are unchanged.

### Added

- **Run summaries now report skipped steps.** `run` (and any `run_flow_with`
  execution) counts steps that never ran — skipped because a dependency failed
  (`--keep-going`) or because a fail-fast run stopped early — and prints
  `Skipped steps: N`. Reporting only; run semantics are unchanged.

## [0.3.6] - 2026-06-27

Makes the agent's autonomous producer-chaining depth tunable. The deterministic
0-LLM verdict core (`argument.rs`) remains byte-identical to 0.3.0, and the
default behavior is unchanged.

### Added

- **`agent run --max-chain-depth N`:** bounds how many levels deep the agent will
  chain producer steps to satisfy a matched tool's missing inputs (default 4).
  `0` disables chaining entirely — a tool runs only if its inputs are directly
  available (a strict, no-synthesis mode); higher values allow longer producer
  ladders.

## [0.3.5] - 2026-06-27

Adds a dry-run plan view of how a flow would execute, plus a second real
literature verifier. The deterministic 0-LLM verdict core (`argument.rs`) remains
byte-identical to 0.3.0.

### Added

- **`flow plan <flow-id>`:** prints the scheduler's wave-by-wave execution plan
  (which steps run in each wave, i.e. the parallelism width and topological
  levels) without running anything — a dry-run view of how `run` / `agent run`
  would execute the flow. `--json` supported.
- **PubMed literature verifier example** (`examples/forage/pubmed_verify.py`): a
  real forage fetch/verify script that queries NCBI E-utilities and flags
  retractions via PubMed's `Retracted Publication` type (PMID-native;
  complements the DOI-native `crossref_verify.py`), resolving DOIs and marking
  PMC full text as open access. The network stays in the script; the core stays
  offline. Ships an offline `--self-test`.

## [0.3.4] - 2026-06-26

Adds opt-in continue-on-error execution. The deterministic 0-LLM verdict core
(`argument.rs`) remains byte-identical to 0.3.0, and the default run behavior is
unchanged (fail-fast).

### Added

- **`--keep-going` (continue-on-error) execution:** `run` / `agent run
  --keep-going` keeps running independent steps when one fails instead of
  stopping at the first failure. A failed step is terminal (not retried) and its
  dependents are skipped, but unrelated ready steps still run — for fan-out flows
  you get every independent result and see all failures in one pass. Default is
  unchanged fail-fast. Holds on both the sequential and parallel paths.

## [0.3.3] - 2026-06-26

Tooling release: working example scripts that make the v0.3.2 literature
auto-verification usable against real services. No engine change — the compiled
runtime (and the 0-LLM verdict core) is functionally identical to 0.3.2.

### Added

- **Crossref literature verifier example** (`examples/forage/crossref_verify.py`):
  a real, working forage fetch/verify script that queries the public Crossref API
  to auto-detect retraction (`update-to` / `RETRACTED` title) and preprint
  publication (`relation.is-preprint-of`), emitting hits whose status AgentFlow
  grades honestly on ingest. The network stays in the script; the core stays
  offline. Ships an offline `--self-test` for the parsing logic. When
  `UNPAYWALL_EMAIL` is set it also resolves precise open-access status per DOI via
  Unpaywall (`is_oa`), falling back to the Crossref heuristic on any lookup
  failure.

## [0.3.2] - 2026-06-26

Completes the literature-evidence honesty lifecycle: a foraged source can move
through preprint → published → retracted with the grade following honestly, and
that status can be verified automatically by an external script while the core
stays offline. The deterministic 0-LLM verdict core (`argument.rs`) remains
byte-identical to 0.3.0.

### Added

- **Preprint publication upgrade:** a foraged observation can record that a
  preprint has since been peer-reviewed/published (`forage observe
  --published-as <id>`). A published preprint is no longer grade-capped — it is
  graded on access like any peer-reviewed source. Combined with retraction
  (which still dominates), a literature source now has a full honesty lifecycle:
  preprint (`Hypothesis`) → published (`LiteratureSupported`) → retracted
  (`Unsupported`).
- **Verified status on ingest:** forage hits JSONL (`forage ingest` /
  `forage fetch --script` / `agent run --forage-script`) may carry `retracted`
  and `published_as`, so an external verify script (PubMed/Crossref/Retraction
  Watch) populates them automatically and AgentFlow grades them honestly on
  ingest — the network stays in the user's script; the core stays offline. Adds
  `examples/forage/verify_status.py` as a template.

## [0.3.1] - 2026-06-25

The agent now builds and runs multi-step research flows itself, executes
independent steps in parallel, and grades foraged literature honestly by source
trust. The deterministic 0-LLM verdict core (`argument.rs`) is byte-identical to
0.3.0 — every change preserves the honesty invariants.

### Added — Autonomous flow construction

- **Deterministic gene-param inference (#93):** a tool param declared
  `infer: gene` is filled from the hypothesis with a gene symbol (0-LLM, mirrors
  `infer: cohort`). The value is recorded as inferred, so an autonomous run stays
  grade-capped and cannot affirm. This let the full autonomous loop run live end
  to end (match tool → fill param → build flow → run real analysis → hand off).
- **Multi-step backward chaining (#94):** when the matched tool needs an input
  type with no available artifact, the agent drafts a producer that outputs it
  and wires `producer.output` into the consumer, applying producers first.
- **Multi-level backward chaining (#95):** the above is now recursive — a
  producer whose own input is unavailable chains another producer (depth-bounded,
  cycle-guarded, all-or-nothing grounding). Proven live with a 3-step
  RawCounts → NormalizedCounts → ExpressionTable → survival ladder the agent built
  itself.
- **Answer-vs-intermediate tool ranking (#96):** for a hypothesis query, tools
  that yield an observation rank strictly ahead of intermediate producers, so a
  keyword-heavy producer can't steal the top branch slot.

### Added — Parallel step execution (#97)

- **`--max-parallel N`** on `run` and `agent run` (default sequential,
  byte-identical). Independent steps in a scheduler wave run their tool
  subprocesses concurrently while preparation and recording stay serial on the
  main thread, so the single SQLite connection is never shared. A consistency
  test asserts parallel runs produce byte-identical outputs to serial; a live
  fan-out of four `sleep 1` steps dropped ~4.0s → ~1.0s.

### Added — Evidence breadth, honestly graded

- **Preprint grading (#99):** foraged full text from known preprint servers
  (bioRxiv/medRxiv/arXiv/SSRN/…) is capped at `Hypothesis` rather than the
  peer-reviewed `LiteratureSupported`, so breadth doesn't inflate confidence.
- **Retraction awareness (#100):** `forage observe --retracted` flags a source;
  retracted sources grade `Unsupported` regardless of access. Because
  `Unsupported`/`Hypothesis` cannot affirm a verdict, preprint and retracted
  evidence can never push a hypothesis to affirmed.

### Fixed

- **Strictly-monotonic entity IDs (#98):** `now_unix_nanos` (consolidated from
  three copies) now returns `max(clock, last + 1)` under a mutex, so back-to-back
  ID minting — newly observable in the parallel wave — can't collide on a UNIQUE
  insert.

### Project

- `scripts/acceptance-session.sh`: a repeatable end-to-end acceptance — from one
  hypothesis the agent builds a multi-level flow, runs it in parallel, foraged
  literature is graded by source trust, and the deterministic verdict honestly
  declines to affirm without observed support.

## [0.3.0] - 2026-06-23

Nextflow-style multi-engine container execution, plus the first real-runtime
validation of the isolated and container backends.

### Added — Multi-engine containers

The container engine is now decoupled from the tool: a tool declares only a
stable `image`, and each run chooses the engine. See
[docs/design/multi-engine-container-design.md](docs/design/multi-engine-container-design.md).

- **Engine seam + selection:** a `ContainerEngine` abstraction (`DockerEngine`,
  `SingularityEngine`) chosen per run via `run`/`agent run`
  `--container-engine docker|podman|singularity|apptainer` and
  `--container-runner <path>` (default docker). Podman reuses the Docker
  CLI-compatible argv; Singularity runs `exec --containall --net --network none
  -B <wd>:<wd> --pwd <wd>` with env forwarded via `SINGULARITYENV_*`.
- **Image-only tools:** container tools declare just `image`; the engine and
  runner come from the run profile.
- **Engine is not in the cache key:** the same image under docker/podman/
  singularity yields identical run identity — the engine only changes *where* a
  tool runs, not *what* it produces.

### Fixed

- **Container input staging:** container backends now stage declared inputs as
  real file copies, not symlinks. A symlink would point into the artifact store,
  which is outside the container's workdir-only mount, leaving a dangling link
  inside the container. Found by the live Docker validation below.

### Validated (live)

- **`isolated-micromamba` — live-proven:** a real run solves a managed env from
  `env_file`, content-addresses + locks it, executes the tool against the env's
  own binaries (real isolation), and reuses it on a second run.
- **`container` + docker — live-proven:** a real `--container-engine docker` run
  executes an image-only tool inside the container with `--network none` and a
  workdir-only mount, producing correct output with full artifact lineage.
- podman/singularity engines remain offline-argv-tested (no local host); see
  `docs/CAPABILITIES.md` §6.6.

## [0.2.0] - 2026-06-22

First cut of the execution engine: per-tool isolation, agent-driven composition
and scheduling, and an OS-level container backend — moving AgentFlow toward the
Nextflow-style model (each tool isolated; tools compose only through declared
I/O) with the agent building and ordering the graph itself.

### Added — Isolated execution engine (P1)

Each tool runs in its own isolated environment, composing only through declared
I/O. See [docs/design/isolated-execution-engine-design.md](docs/design/isolated-execution-engine-design.md).

- **P1.1 — `ToolExecutionBackend` trait:** the per-backend command construction
  is now behind a trait (`runtime/backend.rs`) with a `backend_for` factory —
  the seam future isolated/container backends plug into. Behavior-identical.
- **P1.2 — `isolated-micromamba` backend:** each tool gets a content-addressed
  managed env at `.agentflow/envs/<tool>@<lockhash>` (lockhash = env_file +
  platform), auto-created, locked, and reused; the env lock folds into the run
  cache key (older backends' cache keys stay byte-identical).
- **P1.3 — per-step I/O staging:** declared inputs are staged into the step
  workdir (`workdir/inputs/<port>/`, symlink with copy fallback) and tools see
  only those staged paths — composition flows strictly through declared I/O.
  Logical isolation on local/conda; hard filesystem isolation comes with the
  container backend.
- **Composition proven live:** an end-to-end producer→consumer pipeline on a real
  365-sample TCGA-LIHC slice (`examples/tools/expression_select` →
  `local/survival_assoc`), with the producer's output staged into the consumer's
  workdir and full computed-artifact lineage. Locked by a regression test.

### Added — Agent scheduling & autonomous wiring (P2)

The agent now builds and orders its own composable graphs, not just human-authored
ones. See [docs/design/agent-scheduling-design.md](docs/design/agent-scheduling-design.md).

- **Ready-step scheduler:** a deterministic `StepScheduler` seam orders ready steps
  by how much downstream work they unblock (not authoring order). Scheduling only
  reorders execution — it never changes results (regression-tested).
- **Provenance needs-wiring:** when a step is applied, its `needs` edges are inferred
  from input provenance — an artifact's `source_step_id`, or a `producer.output`
  reference. It only adds edges grounded in real provenance, never invents one.

### Added — Container execution backend (closes #36)

- **`runtime.backend: container`:** runs a tool as `<runner> run --rm --network none
  -v <workdir>:<workdir> -w <workdir> -e AGENTFLOW_*… <image> <command>` — only the
  step workdir is mounted (no artifact-store/host visibility) and no network by
  default. Upgrades P1.3's logical isolation to OS-enforced hard isolation and
  closes the OS-level egress containment issue. Container tools must bake their
  code into the image; covered by offline argv tests (real-Docker validation is a
  later slice — see `docs/CAPABILITIES.md` §6.6).

## [0.1.0] - 2026-06-16

First tagged technical-preview release.

### Added — Tool Evolution Engine (full loop)

The tool registry now converges as it is used, instead of accumulating one-off
scripts per task. Loop: **detect → validate → register candidate → human adopts**.

- **Output-domain validation (AS15):** the agent reads its own finding and rejects
  domain-mismatched results (e.g. a liver-cohort report for a lung hypothesis),
  emitting an `output-domain-mismatch:` apply failure instead of using them.
- **Generalization-candidate detection (AS16):** deterministic, read-only capability
  fingerprinting surfaces specialized tools that could be generalized.
- **Generalization validation gate (AS17, AS17.1, AS17.2):** cohort inference grounded
  in cBioPortal's real study list (preferring `pan_can_atlas`) plus a cross-cohort
  runtime re-check → `promotable` / `rejected`. Never mutates the tool library.
- **Cohort inference in the core run loop (AS19):** a `CohortInferer` core seam
  (Noop default, 0-LLM/0-network) fills cohort/study params declared `infer: cohort`.
  Inferred cohorts are grade-capped so a run cannot affirm on them.
- **Auto-register generalized candidate (AS20):** on `promotable`, deterministically
  derive and register a `<name>_general` candidate at `exploratory` maturity. It
  **never** auto-supersedes; it recommends the human `tools supersede` command.
- **Tool lineage & supersession (AS18):** append-only `tool_superseded` events;
  `agentflow tools supersede <old> --by <new>`. Superseded tools are deprioritized,
  not deleted, keeping lineage traceable.

### Added — Scientific rigor & reporting

- **Citation auditability (AS12):** surfaces verifiable citations and flags uncited
  literature-backed evidence.
- **Methods & Tools reproducibility section (AS13)** in the research report.
- **Mechanism-probing child hypothesis (AS14)** spawned on affirmation.
- **Autonomous source discovery + question-aware fundamental-gap handoff (AS7–AS8.2).**

### Added — Security

- **Runtime egress guard (#49):** in-process `sitecustomize` cooperative guard for
  synthesized Python tools (blocks private/loopback/link-local/metadata/CGNAT).
- **Deployment-level egress containment recipe (#53):** `docs/ops/egress-containment.md`
  (Docker `--network none`, bridge + nftables allowlist, netns + veth + nft) and
  `scripts/verify-egress-policy.sh`.
- **Capability/invariant/security-boundary overview:** `docs/CAPABILITIES.md`.

### Fixed — Security (pre-release audit, Opus 4.8 + Codex)

A dual-engine pre-release audit confirmed the honesty/determinism invariants hold
(verdict determinism, grade-cap, allowlist robustness, path safety) and surfaced
three issues that defeated controls claimed to hold — all now fixed:

- **HIGH — inline shell/interpreter validation bypass:** a hostile/auto-synthesized
  tool YAML could pass `runtime.command` validation yet execute arbitrary shell via
  an `env` wrapper or combined/long flags (`env sh -c`, `sh -ec`, `bash --noprofile -c`).
  `is_inline_interpreter_command` now unwraps `env` and matches those forms.
- **MEDIUM — probe subprocess proxy/env trust:** source-discovery probe subprocesses
  inherited `HTTP(S)_PROXY` / `*_API_KEY` and exempted proxy hosts from private-IP
  checks (SSRF to metadata via a hostile proxy). Now `env_clear()` + `ProxyHandler({})`.
- **MEDIUM — DNS-pin missed IPv4-mapped / NAT64 IPv6:** `::ffff:169.254.169.254` and
  `64:ff9b::/96` bypassed the private-IP classifiers via a hostile DNS AAAA answer.
  All three Python classifiers now unwrap mapped/NAT64 addresses.

Lower-severity hardening follow-ups are also fixed:

- **#57 resource-exhaustion caps:** named byte caps on artifact/result/YAML reads
  and truncated stdout/stderr capture, returning a clear error instead of
  exhausting memory.
- **#58 artifact reference safety:** reference-mode imports resolving outside the
  project root are rejected by default (opt-in via `--allow-external-reference`);
  list/inspect no longer leak full host paths.
- **#59 runtime-tool egress guard:** registered `synth`-namespace tools now run
  under the same cooperative egress guard applied at validation time.

### Changed

- `ToolSpec::spec_hash()` is now the single source of truth for the stored spec hash.

### Project

- Added `LICENSE` (MIT), `SECURITY.md`, and this changelog.
