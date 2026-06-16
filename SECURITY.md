# Security Policy

## Reporting a vulnerability

Please report security issues **privately**, not in public issues or pull requests.

- Use GitHub's [private vulnerability reporting](https://github.com/dxsbiocc/AgentFlow/security/advisories/new) ("Report a vulnerability"), or
- Email the maintainer at **dxsbiocc@gmail.com** with a clear description and reproduction steps.

Please include: affected version/commit, impact, reproduction, and any logs (with secrets redacted). We aim to acknowledge within a few days. Please give us reasonable time to ship a fix before public disclosure.

## Threat model & security posture

AgentFlow is a **CLI-first local research runtime**. It executes locally declared tools and, when explicitly enabled, LLM-synthesized tools and autonomous source discovery. Security is **layered defense-in-depth**, not a single mechanism. The authoritative, detailed breakdown lives in [docs/CAPABILITIES.md](docs/CAPABILITIES.md) §2 (honesty invariants) and §6 (security layering); the deployment-level egress recipe lives in [docs/ops/egress-containment.md](docs/ops/egress-containment.md).

Key properties:

- **Decision determinism.** The verdict exit (`crates/agentflow-core/src/argument.rs`) is an invariant **0-LLM / 0-network** boundary. LLMs may participate in upstream tool synthesis, semantic fit, output grounding, and cohort selection, but never in evidence scoring or the final verdict.
- **No-fabrication.** Synthesized tools must read real inputs or real public sources; they fail rather than emit default/illustrative values. Input-sensitivity and runtime gates reject tools that don't register honestly.
- **Grade cap.** Unverified/synthesized tools and inferred parameters cannot independently drive an `affirmed` verdict.
- **Egress controls.** Autonomous source discovery probes only an allowlist of public scientific `http(s)` domains, with DNS-pinning, no-redirect, and private/loopback/link-local/metadata/CGNAT rejection. Synthesized Python tools also run under an in-process `sitecustomize` egress guard.

## Known limitations (not vulnerabilities by themselves — by design, documented)

These are explicit, documented boundaries. Please **do not** file these as new vulnerabilities; they are tracked design limits:

- **The in-process Python egress guard is cooperative, not anti-tamper.** A synthesized script has a full Python runtime and could un-patch `socket`, swap the interpreter, or use native extensions to bypass it. Real containment against an anti-tamper adversary requires an OS boundary — container / VM / netns + nftables / Kubernetes NetworkPolicy. See [docs/ops/egress-containment.md](docs/ops/egress-containment.md) for the default-deny deployment recipe.
- **macOS seatbelt cannot CIDR-filter.** The `sandbox-exec` layer blocks loopback SSRF but cannot precisely deny RFC1918 / metadata by CIDR; on Linux CI / non-macOS it fails open to avoid breaking legitimate tool execution.
- **LLM-synthesized tools are not guaranteed scientifically correct.** Fixtures, input-sensitivity, runtime gates, and output grounding reduce self-deception but do not replace expert review.
- **No resource quotas / redaction policy / parallel-execution sandboxing yet** (see README "Explicitly Not Supported Yet").

If you find a way to **bypass a control that is claimed to hold** (e.g. exfiltrate evidence into the deterministic verdict, reach a private/metadata address despite the allowlist + DNS-pin on the *system-controlled* probe path, or coerce an `affirmed` verdict from unverified/inferred inputs), that **is** a vulnerability — please report it.

## Supported versions

This is a pre-1.0 technical preview. Only the latest `main` receives security fixes.
