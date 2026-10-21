# Proposal

## Why

Users must be able to open and edit their notes from any browser with nothing installed, and edits must appear in the native apps and vice versa. The browser client is deliberately **thin**: it decrypts the vault in memory after login and leaves no keys or plaintext behind when the tab closes, which keeps borrowed or shared computers safe.

## What Changes

- Build `notebook-core` to WebAssembly through a new thin bindings crate (`notebook-wasm`, `wasm-bindgen`), using the in-memory storage implementation and the wasm sync transport. `notebook-core` itself stays free of JS bindings.
- Reuse the shared frontend (layout, editor, sidebar) with a WASM-backed core adapter instead of Tauri IPC. The adapter is chosen at build time, so the browser bundle carries no Tauri code and the Tauri bundle carries no WASM.
- **Session model**:
  - Login (OPAQUE), then fetch and unwrap the key bundle, bootstrap the vault, decrypt in memory, and build the tag and search indexes, with visible progress.
  - Nothing persisted: no keys, tokens, plaintext, or note data in localStorage, sessionStorage, IndexedDB, OPFS, Cache Storage, or cookies set by the client, and no service worker. Only an allowlisted set of non-sensitive UI preferences (vim toggle, theme) may be stored locally. The one deliberate exception is a plaintext markdown export zip the user explicitly starts, delivered as a browser download.
  - Closing, refreshing, or navigating away from the tab ends the session, including pages restored from the back/forward cache. The user re-enters the passphrase. "Lock" in the browser is the same as signing out.
- **Connectivity**: live sync over WebSocket. On disconnect, buffer unsent updates in memory, show an "unsynced changes" indicator, retry with backoff, and warn on tab close while updates are pending.
- Browser side of `markdown-export`: a `notebook-wasm` `export_zip` binding over the core's `plan` and `write_zip`, and the adapter's `exportNotes` triggering a Blob download.
- Register and recover with a recovery key from the browser, using the same account flows as the desktop app. The invite-code field arrives with `invite-codes`, which modifies those shared flows.
- Served by `notebook-server` as hashed static assets, with Subresource Integrity on the entry script and the WASM module, under the server's existing strict CSP (no relaxation).
- Document the inherent limit of browser E2EE: users trust the server to deliver unmodified client code on each load.

## Capabilities

### New Capabilities
- `web-client`: Browser-specific behavior: in-memory-only session lifecycle, no persistence of sensitive data, offline buffering and unload warnings, bundle integrity and CSP compatibility, and the documented trust model.

### Modified Capabilities
<!-- None. `app-shell` behavior applies as-is. Credential storage requirements in `app-shell` are native-only by design. `server-hosting` already serves the bundle with the required caching and CSP. -->

## Impact

- New crate `crates/notebook-wasm` (`cdylib`, `wasm-bindgen`, `wasm-bindgen-futures`, `js-sys`) wrapping `notebook-core` sessions, sync, documents, tags, and search for JS. Built with a pinned `wasm-bindgen-cli`.
- Frontend: a WASM implementation of the platform adapter defined by `desktop-app-shell`, a browser build mode in Vite (output `web/dist-web/`), and a small Vite plugin that adds SRI hashes.
- `notebook-server`: no code change. Operators point `static_dir` at the browser build; `docs/deployment.md` is updated.
- Depends on: `crdt-note-documents`, `local-sqlite-storage` (in-memory `Storage`), `opaque-accounts`, `sync-server`, `sync-client-engine` (wasm transport), `note-editor-component`, `desktop-app-shell` (shared UI and adapter interface), `markdown-export` (`plan`, `write_zip`, adapter `exportNotes`).
- Tests: `wasm-bindgen-test` for the bindings and Playwright end-to-end tests against a real server (storage audit, CSP violations, cross-client sync, unload warning).
- Performance: decrypting the whole vault at login is fine at personal scale. An encrypted index snapshot is a possible later optimization.
