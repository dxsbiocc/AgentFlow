# Changelog

All notable changes to AgentFlow are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project is a
pre-1.0 technical preview; until a tagged release, everything lands under
**Unreleased**.

## [Unreleased]

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

Lower-severity hardening items are tracked as follow-ups (#57 resource-exhaustion
caps, #58 artifact reference mode, #59 runtime-tool egress guard).

### Changed

- `ToolSpec::spec_hash()` is now the single source of truth for the stored spec hash.

### Project

- Added `LICENSE` (MIT), `SECURITY.md`, and this changelog.
