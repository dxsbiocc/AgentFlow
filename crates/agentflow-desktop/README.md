# AgentFlow Desktop

A read-only desktop viewer for AgentFlow projects, built with Tauri + React +
TypeScript. See [`docs/design/desktop-ui-design.md`](../../docs/design/desktop-ui-design.md)
for the design and scope (this first slice: project picker + status overview
only — no run-triggering, tool registration, or flow mutation from the UI).

## Development

```bash
npm install
npm run tauri dev
```

## Build

```bash
npm install
npm run tauri build
```

The Rust IPC layer lives in `src-tauri/` and calls into the
`agentflow-desktop-api` crate, which wraps `agentflow-core` read-only.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
