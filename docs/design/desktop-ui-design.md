# Design: desktop UI (read-only viewer) via Tauri

Status: design + first-slice implementation. Slice 1 (project picker +
status overview) is complete; later slices are planned but not built.

## 1. Motivation

AgentFlow has been CLI-only since its inception. An assessment of
public-release readiness concluded the runtime and agent-control-layer
surface is solid, but the project has zero interface layer — everything is
a terminal command, which is a real barrier to demoing and promoting the
project beyond people already comfortable in a shell. This is not an effort
to replace the CLI: the CLI remains the scripting/automation surface and the
source of truth for all mutating operations. The desktop app is a companion
**viewer**, positioned to make the project's honesty invariants (deterministic
verdict, grade-cap, no-fabrication, human-in-loop) visible and demoable
without requiring a terminal.

## 2. Architectural boundary this introduces

This is the **first JavaScript/Node toolchain** (no `package.json` existed
anywhere in this repo before this slice) and the **first GUI/webview crate**
in a repository that has been 100% Rust-only. This is a deliberate, scoped
addition, not something to hide: it is confined to two new crates
(`agentflow-desktop-api`, `agentflow-desktop`); `agentflow-core` and
`agentflow-cli` keep their existing dependency lists unchanged (the only
core-adjacent change is a `#[derive(Serialize)]` on `ProjectSummary` — a
data-shape annotation using a dependency core already has, not a new one).

## 3. Why Tauri, and why IPC commands, not an HTTP server

A native desktop shell was chosen over a web dashboard or TUI. Tauri's
built-in IPC bridge (`#[tauri::command]`, called from the frontend via
`@tauri-apps/api/core`'s `invoke()`) is used instead of an embedded HTTP
server: this is a local, single-user desktop app, so there is no need for
port binding, CORS, or a network-facing surface at all. Direct in-process
Rust function calls from the webview are both simpler to build and strictly
smaller attack surface than standing up axum/tokio for a purely local
IPC channel.

## 4. Read-only scope

This entire feature, across all slices covered by this document, exposes
zero mutating `ProjectStore` calls through any IPC command. Triggering a
run, registering a tool, approving a flow, cancelling a detached job, etc.
remain CLI-only. If a future slice needs a write path, it gets its own
design review — it is not assumed here.

## 5. Crate layout

- **`agentflow-desktop-api`** — a thin, GUI-framework-agnostic Rust facade
  over `agentflow-core`. No Tauri dependency at all, so it stays
  independently unit-testable and reusable if a second frontend ever
  appears (e.g. a future web dashboard). It re-exports existing core types
  directly (e.g. `pub use agentflow_core::storage::ProjectSummary;`) rather
  than hand-mirroring parallel DTOs, and only defines genuinely new types
  where core has no matching shape (e.g. the aggregate `ProjectOverview`).
- **`agentflow-desktop`** — the Tauri v2 app. `crates/agentflow-desktop/src-tauri/`
  is the actual Cargo package (the workspace member); `crates/agentflow-desktop/`
  itself is the Vite/React/TypeScript frontend project root. This matches
  Tauri's own tooling conventions (`npm create tauri-app` scaffolding, `cargo
  tauri dev`/`build`) with zero path surgery.

## 6. First slice (shipped)

- One IPC command: `open_project(path: String) -> Result<ProjectOverview, String>`.
- `agentflow-desktop-api::open_project_overview` composes `ProjectStore::open`
  + `.summary()` + the new `.count_flows()` + `.list_tools().len()` +
  `.list_runs(None).len()` + `.list_artifacts().len()` into one
  `ProjectOverview` DTO.
- Frontend: a project picker (native folder dialog via
  `@tauri-apps/plugin-dialog`) and a status/overview screen rendering the
  DTO. No router, no state-management library — two screens, one linear
  transition (pick → view), plain `useState` in `App.tsx`.
- Error handling: `StorageError`'s existing `Display` messages (e.g.
  `NotProject` → `"not an AgentFlow project: missing <path>/.agentflow/project.db"`)
  are surfaced directly via `.map_err(|e| e.to_string())` — no new
  error-mapping layer.

## 7. Phased slice plan (not built yet)

- **Slice 2**: flow list + flow detail (`inspect_flow`).
- **Slice 3**: run history + attempt detail (`list_runs`, `inspect_run_or_attempt`, `read_logs`).
- **Slice 4**: artifact + cache browsers (`list_artifacts`, `list_cache_entries`, `cache_explain_*`).

Each slice adds a sibling `commands/<domain>.rs` module to
`agentflow-desktop`/`agentflow-desktop-api`, in the same shape as
`open_project` — no architecture rework is anticipated. Packaging desktop
binaries for release (a new `release.yml` build job, code signing, installer
bundling) is deferred until there's enough UI to justify shipping it.

## 8. Invariants / risks / open questions

- `argument.rs` (the 0-LLM verdict engine) is untouched; this is execution
  UI only.
- `agentflow-core`'s and `agentflow-cli`'s own `Cargo.toml` dependency lists
  are unchanged.
- No mutating IPC command in this document's scope, in any slice.
- **CI gap, fixed in this slice's PR**: Tauri v2 needs `libwebkit2gtk-4.1-dev`,
  `libappindicator3-dev`, `librsvg2-dev`, `patchelf` to compile on Linux —
  `ci.yml` gained an install step for these before the clippy/test steps.
- No automated E2E/browser test exists for the Tauri+React app yet (no
  prior JS testing convention in this repo — Playwright/Vitest would be a
  separate, later decision).
