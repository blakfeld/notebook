# Proposal

## Why

Notebook is a greenfield, end-to-end encrypted notes app spanning a Rust core, a public sync server, Tauri desktop/mobile apps, and a thin browser client. Every later change needs a shared repository layout, build tooling, and CI to land into, so that structure must exist first.

## What Changes

- Create a Cargo workspace with three crates:
  - `notebook-core`: platform-independent library (must build for native targets and `wasm32-unknown-unknown`).
  - `notebook-server`: axum-based server binary (empty stub).
  - `notebook-app`: Tauri 2 application crate (empty window stub).
- Create a TypeScript frontend package (Svelte or Solid, settled in design) shared by the Tauri webview and the browser build, using Vite.
- Add formatting and lint configuration (`rustfmt`, `clippy`, TS linter/formatter).
- Add CI that builds `notebook-core` for both native and `wasm32`, runs Rust and TS tests, and lints.
- Add a top-level README describing the layout and how to run each piece.

## Capabilities

### New Capabilities
<!-- None: this change is tooling and project structure only; no user-observable behavior. -->

### Modified Capabilities
<!-- None. -->

This change sets `skip_specs: true` because it introduces no spec-level behavior.

## Impact

- New repository layout used by every subsequent change.
- Dependencies: Rust stable toolchain, `wasm32-unknown-unknown` target, Tauri 2 CLI, Node/pnpm.
- CI provider configuration (GitHub Actions assumed).
- Ensuring `notebook-core` builds for `wasm32` from day one prevents native-only dependencies from creeping into the core.
