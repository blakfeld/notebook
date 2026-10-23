# Notebook

An encrypted, local-only notes app for the desktop: a Rust core and a Tauri
app around a Svelte frontend. There are no accounts, logins, or servers, and
the app makes no network requests. Notes are encrypted on disk with a vault
key kept in the OS keychain. A recovery key, shown once when the notebook is
created, opens the notebook when the keychain doesn't have it.

## Layout

```
Cargo.toml              Cargo workspace (shared deps and lints)
rust-toolchain.toml     pinned Rust toolchain, incl. wasm32-unknown-unknown
crates/
  notebook-core/        platform-independent library (native + wasm32)
  notebook-storage-sqlite/  the local SQLite store
  notebook-app/         Tauri 2 desktop app around web/
web/                    Svelte 5 + Vite + TypeScript frontend
  src/lib/              components and helpers
  src/platform/         host adapter (Tauri IPC)
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

The desktop app in dev mode. It starts the web dev server itself:

```sh
cd crates/notebook-app && cargo tauri dev
```

On first launch it offers to create a notebook, shows the recovery key once,
and opens after you confirm you saved it. Later launches open straight into
the notes. Without an OS secure store (for example, Linux with no Secret
Service), the app asks for the recovery key at each launch instead of storing
the vault key in a file.

The frontend alone, in a plain browser at <http://localhost:5173>, runs on an
in-memory demo with sample notes (dev builds only):

```sh
cd web && npm run dev
```

### Upgrading from the server-backed version

Earlier versions signed in to a sync server. Before upgrading a device, let
it finish syncing (the indicator shows "Synced") so every note is on the
device. On first launch, the new version converts the device's notebook in
place. It uses the stored vault key, or asks once for the old passphrase if
the app was locked. It then shows a new recovery key. Save it: the old
recovery key no longer opens anything. If the old passphrase is lost, "Start
over" keeps the old database beside the new one (`vault.sqlite3.legacy-*` in
the app's data directory) and creates an empty notebook.

A later version also upgrades the notebook's storage in place and drops the
sync bookkeeping the old version kept. Every note and edit is kept, but take
a backup (or a markdown export from Settings) before installing it, in case
something goes wrong.

### Shutting down an old sync server

The server is no longer part of this repository and nothing in the app talks
to it. Keep it running until every device has upgraded (each one needs its
notes on the device first) and has been backed up or exported. Then stop the
service and delete its data directory: it holds only ciphertext that the
upgraded devices no longer read. The last version of the server's code and
its deployment guide are in git history (`crates/notebook-server` and
`docs/deployment.md` before the `retire-sync-server` change).

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
