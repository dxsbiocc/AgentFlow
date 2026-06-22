# Changelog

All notable changes to AgentFlow are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). This is a pre-1.0
technical preview; the public API and CLI surface may change between minor versions.

## [Unreleased]

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
