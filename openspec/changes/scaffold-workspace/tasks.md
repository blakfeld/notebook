# Tasks

## 1. Repository and toolchain

- [x] 1.1 Run `git init` and add a `.gitignore` covering `target/`, `web/node_modules/`, `web/dist/`, `.DS_Store`, and editor files; verify `git status` shows no build output after later steps
- [x] 1.2 Add `rust-toolchain.toml` pinning a specific stable release (≥ 1.85) with `wasm32-unknown-unknown`, `rustfmt`, and `clippy`; verify `rustc --version` in the repo reports the pinned version, not 1.62.1
- [x] 1.3 Create the root `Cargo.toml` workspace (resolver 3, members `crates/*`, `[workspace.package]` with edition 2024 and `UNLICENSED` placeholder license, shared `[workspace.lints]`, `[workspace.dependencies]`) and `rustfmt.toml`; verify `cargo metadata --no-deps` succeeds

## 2. notebook-core

- [x] 2.1 Create `crates/notebook-core` as a library with `#![forbid(unsafe_code)]`, workspace lints, and no dependencies; verify `cargo build -p notebook-core` succeeds
- [x] 2.2 Add `pub fn version() -> &'static str` returning `CARGO_PKG_VERSION`, with a unit test asserting it is non-empty and matches the crate version; verify `cargo test -p notebook-core` passes
- [x] 2.3 Verify `cargo build -p notebook-core --target wasm32-unknown-unknown` succeeds

## 3. notebook-server stub

- [x] 3.1 Create `crates/notebook-server` binary depending on `notebook-core`, `tokio` (rt-multi-thread, macros), `axum`, and `tracing`/`tracing-subscriber`; `main` logs `notebook-server <version>` using `notebook_core::version()` and exits; verify `cargo run -p notebook-server` prints the version line and exits 0

## 4. Web frontend (Svelte 5 + Vite)

- [x] 4.1 Scaffold `web/` as a Vite + Svelte 5 + TypeScript SPA (no SvelteKit) using npm, with `src/lib/` and `src/platform/` directories; verify `npm run build` in `web/` produces `web/dist/index.html`
- [x] 4.2 Add Prettier (`prettier-plugin-svelte`), ESLint flat config (`typescript-eslint`, `eslint-plugin-svelte`), and `svelte-check`, with npm scripts `lint`, `format:check`, `check`; verify each script runs cleanly
- [x] 4.3 Add Vitest with a sample unit test for a small `src/lib` helper, plus npm script `test`; verify `npm test` passes
- [x] 4.4 Build the placeholder page: title "Notebook" and a core-version line that calls `invoke('core_version')` via `@tauri-apps/api` when running in Tauri and shows "browser build" otherwise; verify `npm run dev` renders the page in a browser without console errors

## 5. notebook-app (Tauri 2)

- [x] 5.1 Create `crates/notebook-app` as a Tauri 2 crate (`tauri.conf.json` with `frontendDist: ../../web/dist`, `devUrl: http://localhost:5173`, `beforeDevCommand`/`beforeBuildCommand` running the `web/` npm scripts, default capabilities, generated icons); verify `cargo build -p notebook-app` succeeds after `web/dist` is built
- [x] 5.2 Register a `core_version` Tauri command returning `notebook_core::version()`; verify `cargo tauri dev` (from `crates/notebook-app`) opens a window showing "Notebook" and the core version

## 6. Local checks, CI, and docs

- [x] 6.1 Add `scripts/check.sh` that runs, in order: web install/lint/format check/svelte-check/test/build, then `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and the wasm32 core build; verify it exits 0 on a clean tree and non-zero when a formatting error is introduced
- [x] 6.2 Add `.github/workflows/ci.yml` with jobs `web` (uploads `web/dist`), `rust` (needs `web`; downloads dist, installs Tauri Linux deps, rust-cache, fmt/clippy/test), and `wasm` (core wasm32 build); verify with `actionlint` (or a successful run once a remote exists)
- [x] 6.3 Write `README.md` covering the layout, prerequisites (rustup, Node/npm, Tauri system deps), and how to run the server stub, the Tauri app in dev, the web dev server, and `scripts/check.sh`; verify each documented command runs as written on a fresh checkout

## Workflow follow-up

- Commit the scaffold as the first commit once you've reviewed it.
- Archive the change after review; it has no specs to sync (`skip_specs: true`).
- Decide the license (design Open Questions) before any public release.
