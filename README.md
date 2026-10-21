# Notebook

An end-to-end encrypted notes app: a Rust core shared by a sync server, Tauri
desktop/mobile apps, and a browser client.

## Layout

```
Cargo.toml              Cargo workspace (shared deps and lints)
rust-toolchain.toml     pinned Rust toolchain, incl. wasm32-unknown-unknown
crates/
  notebook-core/        platform-independent library (native + wasm32)
  notebook-server/      axum server binary (stub)
  notebook-app/         Tauri 2 app (desktop window around web/)
web/                    Svelte 5 + Vite + TypeScript frontend
  src/lib/              components and helpers
  src/platform/         host adapter (Tauri IPC today)
scripts/check.sh        every CI check, run locally
openspec/               specs and planned changes (see TODO.md)
```

## Prerequisites

- [rustup](https://rustup.rs). The pinned toolchain in `rust-toolchain.toml`
  (with `wasm32-unknown-unknown`, `rustfmt`, `clippy`) installs automatically
  on the first `cargo` command in this repo.
- Node.js 26 with npm.
- Tauri system dependencies for your OS, see
  <https://v2.tauri.app/start/prerequisites/>. On Debian/Ubuntu:

  ```sh
  sudo apt-get install libwebkit2gtk-4.1-dev libxdo-dev libssl-dev \
    libayatana-appindicator3-dev librsvg2-dev
  ```

- The Tauri CLI: `cargo install tauri-cli --version "^2" --locked`.

Install the frontend dependencies once:

```sh
cd web && npm ci
```

## Running

Web dev server (plain browser build, shows "browser build"), at
<http://localhost:5173>:

```sh
cd web && npm run dev
```

Tauri app in dev mode (starts the web dev server itself, shows the core
version):

```sh
cd crates/notebook-app && cargo tauri dev
```

Server stub (logs its version and exits):

```sh
cargo run -p notebook-server
```

`notebook-app` embeds `web/dist` at compile time, so build the frontend before
any `cargo build`, `clippy`, or `test` that includes it:

```sh
(cd web && npm run build) && cargo build --workspace
```

## Checks

Run everything CI runs, in the same order:

```sh
scripts/check.sh
```
