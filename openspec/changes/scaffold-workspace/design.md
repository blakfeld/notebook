# Design

## Context

See proposal.md (Why). The repository is empty apart from `openspec/` and `TODO.md`, and isn't yet a git repository. The local toolchain at planning time is `rustc 1.62.1` (too old for Tauri 2 and edition 2024), Node 26 with `npm`, and no `pnpm`. Later changes set these constraints on the layout:

- `notebook-core` is consumed natively (server, Tauri app) and as WebAssembly (browser client, `browser-web-client`).
- One TypeScript frontend serves two hosts: the Tauri webview and a browser bundle served by `notebook-server`.
- CodeMirror 6 (an imperative library) will be wrapped by the UI (`note-editor-component`).

## Goals / Non-Goals

**Goals:**
- A layout every later change can land into without moving directories.
- A pinned, reproducible toolchain.
- A `wasm32-unknown-unknown` build of `notebook-core` enforced by CI from the first commit.
- An end-to-end smoke path: the Tauri window renders the Svelte UI, which calls a Tauri command backed by `notebook-core`.
- One-command local checks matching CI (`scripts/check.sh`).

**Non-Goals:**
- Any product behavior (notes, crypto, HTTP endpoints, routing). The server stub doesn't serve requests yet; that's `server-foundation`.
- The browser build target and the platform adapter (Tauri IPC vs WASM). That's `browser-web-client`, though the layout leaves room for it.
- Mobile targets, code signing, release packaging, and deployment.

## Decisions

### Repository layout

```
notebook/
  Cargo.toml              workspace root (resolver 3, shared deps + lints)
  rust-toolchain.toml     pinned stable + wasm32-unknown-unknown, rustfmt, clippy
  crates/
    notebook-core/        lib; no platform deps; forbid(unsafe_code)
    notebook-server/      bin; axum + tokio (stub main)
    notebook-app/         Tauri 2 crate (tauri.conf.json, capabilities/, icons/)
  web/                    Svelte 5 + Vite + TypeScript frontend
  .github/workflows/ci.yml
  scripts/check.sh        runs every CI check locally, in CI order
  README.md
```

- **Tauri crate under `crates/`, not `web/src-tauri`.** Tauri's default puts `src-tauri` inside the frontend, but here the frontend is shared and the Tauri app is only one host. Keeping every crate under `crates/` makes the workspace uniform. `tauri.conf.json` points `build.frontendDist` at `../../web/dist` and `build.devUrl` at the Vite dev server.
- **Alternative considered:** a flat layout (`core/`, `server/`, `app/`). Rejected because `crates/` keeps the root uncluttered as crates are added (e.g. a possible `notebook-storage-sqlite`).

### Toolchain

- `rust-toolchain.toml` pins a specific stable release (current stable at implementation time, ≥ 1.85 for edition 2024) with the `wasm32-unknown-unknown` target and `rustfmt`/`clippy` components. rustup installs it automatically, which also fixes the outdated local `rustc`.
- Edition 2024 across all crates. `[workspace.package]` holds the shared edition, version, and license, and `[workspace.lints]` sets shared lint levels (`unsafe_code = "forbid"` in core, `clippy::all = "warn"`, with CI denying warnings).
- **npm, not pnpm.** There's a single frontend package and npm is already installed. pnpm would add a tool without a clear benefit. This can be revisited if the frontend becomes several packages.

### Frontend: Svelte 5 + Vite, no SvelteKit

- Plain Vite + Svelte 5 (runes) + TypeScript as a single-page app.
- **Alternative considered:** SvelteKit with `adapter-static`. Rejected because no SSR or server routes are needed (Tauri and axum both serve static files). SvelteKit's routing and filesystem conventions add weight, and it conflicts with the "same bundle in two hosts" model. A small client router can be added in `desktop-app-shell` if needed.
- Source layout leaves room for later changes: `web/src/lib/` for components, `web/src/platform/` for the future host adapter.

### Linting, formatting, testing

- Rust: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- TS/Svelte: Prettier (with `prettier-plugin-svelte`), ESLint (flat config with `typescript-eslint` and `eslint-plugin-svelte`), `svelte-check` for type checking, and Vitest for unit tests.
- **Alternative considered:** Biome. It's faster and needs one tool instead of two, but its Svelte support is still partial. Rejected for now.

### Smoke path

- `notebook-core` exposes `pub fn version() -> &'static str` (from `CARGO_PKG_VERSION`) with a unit test. It's a placeholder that proves linking, not an API.
- `notebook-app` registers a `core_version` Tauri command that returns it. The Svelte page shows "Notebook" plus that version, proving the webview → IPC → core path.
- `notebook-server` is a `tokio` main that logs its version via `notebook-core` and exits. It depends on `axum` so the dependency graph and compile times are realistic from the start.

### CI (GitHub Actions)

Three parallel jobs on `ubuntu-latest`:

1. **web**: `npm ci`, lint, `svelte-check`, `vitest run`, `vite build`, and upload `web/dist` as an artifact.
2. **rust**: depends on **web** (downloads `web/dist`, because Tauri's `generate_context!` needs `frontendDist` to exist at compile time). Installs Tauri's Linux system packages (`libwebkit2gtk-4.1-dev`, etc.), then runs fmt, clippy, and tests. Uses `Swatinem/rust-cache`.
3. **wasm**: `cargo build -p notebook-core --target wasm32-unknown-unknown`. This is the guard that keeps the core platform-independent.

macOS/Windows matrix and Tauri bundling are deferred until there's something worth shipping.

## Risks / Trade-offs

- [Tauri compile requires `web/dist` to exist] → CI orders **rust** after **web**. Locally, `scripts/check.sh` and the README build the frontend first. A committed placeholder `dist/` was rejected because it hides stale-build mistakes.
- [Linux CI needs WebKitGTK system packages, which are slow to install] → cache apt packages, or accept ~1 min. Only the **rust** job pays it.
- [Edition 2024 / new toolchain may break with older crates] → the pin makes upgrades deliberate. Every dependency is new, so the risk is low.
- [A wasm guard only on `notebook-core` won't catch wasm-incompatible deps that arrive behind feature flags] → later changes keep native-only code behind explicit `native` features or in separate crates, and the CI command can add `--no-default-features` once features exist.
- [npm is chosen over pnpm and may be revisited] → a single package makes switching cheap.

## Migration Plan

Greenfield, so nothing to migrate. Initialize git first so the scaffold is the first commit. Rollback means deleting the generated files.

## Open Questions

- License choice for `[workspace.package]` (e.g. MIT/Apache-2.0 vs AGPL for the server). It's deferrable; use `UNLICENSED`/proprietary placeholder until decided.
