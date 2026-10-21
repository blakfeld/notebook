# Proposal

## Why

Users must be able to open and edit their notes from any browser with nothing installed, and edits must appear in the native apps and vice versa. The browser client is deliberately **thin**: it decrypts the vault in memory after login and leaves no keys or plaintext behind when the tab closes, which keeps borrowed or shared computers safe.

## What Changes

- Build `notebook-core` to WebAssembly (`wasm-bindgen`) with the in-memory storage implementation and the wasm sync transport.
- Reuse the shared frontend (layout, editor, sidebar) with a WASM-backed core adapter instead of Tauri IPC.
- **Session model**:
  - Login (OPAQUE), then fetch and unwrap the key bundle, bootstrap the vault, decrypt in memory, and build the tag and search indexes.
  - Nothing persisted: no keys, plaintext, or note data in localStorage, IndexedDB, or OPFS. Only non-sensitive UI preferences (e.g. vim toggle, theme) may be stored locally.
  - Closing or refreshing the tab ends the session. The user re-enters the passphrase.
- **Connectivity**: live sync over WebSocket. On disconnect, buffer unsent updates in memory, show an "unsynced changes" indicator, retry with backoff, and warn on tab close while updates are pending.
- Register with an invite code and recover with a recovery key from the browser.
- Served by `notebook-server` as hashed static assets, with Subresource Integrity and a strict CSP.
- Document the inherent limit of browser E2EE: users trust the server to deliver unmodified client code on each load.

## Capabilities

### New Capabilities
- `web-client`: Browser-specific behavior: in-memory-only session lifecycle, no persistence of sensitive data, offline buffering and unload warnings, and delivery integrity of the client bundle.

### Modified Capabilities
<!-- None. `app-shell` behavior applies as-is. Credential storage requirements in `app-shell` are native-only by design. -->

## Impact

- `notebook-core`: wasm bindings layer (`wasm-bindgen`, `getrandom` js feature, `web-sys`).
- Frontend: platform adapter abstraction (Tauri vs WASM), and a browser build target in Vite.
- `notebook-server`: serves the built bundle (via `server-hosting` static serving).
- Performance: decrypting the whole vault at login is fine at personal scale. An encrypted index snapshot is a possible later optimization.
