# Design

## Context

See proposal.md (Why). Requirements are in `specs/web-client/spec.md`.

Current state:

- `notebook-core` builds for `wasm32-unknown-unknown` (CI `wasm` job) with `getrandom`'s `wasm_js` backend. It has `crypto` and `tags`; `document` (Yrs), `storage` (incl. in-memory), `sync` (with a wasm transport), and client-side OPAQUE arrive with `crdt-note-documents`, `local-sqlite-storage`, `sync-client-engine`, and `opaque-accounts`. The core forbids `unsafe` and has no JS bindings.
- `web/` is a Svelte 5 + Vite app whose only platform code is `web/src/platform/index.ts` (a Tauri `invoke` with an `isTauri()` guard). `desktop-app-shell` turns this into a platform adapter interface plus a Tauri implementation; `note-editor-component` defines the editor's narrow document interface (load, emit local updates, apply remote updates). `web/dist/` is consumed by Tauri.
- `notebook-server` (from `server-foundation`) serves a configured `static_dir`, with `/assets/*` immutable and everything else `no-cache`, and sends `Content-Security-Policy: default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' data: blob:; font-src 'self'; connect-src 'self'; manifest-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`. Its design anticipated this change might relax `style-src`; this design avoids that.

## Goals / Non-Goals

**Goals:**
- One core, one UI: the browser runs the same Rust logic (crypto, CRDT, sync, tags, search) and the same Svelte components as the desktop app; only the adapter differs.
- Keys and plaintext exist only in the tab's JS/WASM heap and die with the page.
- Ship under the server's current CSP unchanged.

**Non-Goals:**
- Offline-capable or installable PWA (would need persistence and a service worker, both forbidden).
- Defending against a malicious server serving modified code (documented limit, not solvable in-browser).
- Moving the core into a Web Worker (possible later; see Decisions).
- Idle auto-lock in the browser (closing the tab is the lock). Export format, naming, and the zip writer (owned by `markdown-export`; this change only binds and downloads it).
- The invite-code field (owned by `invite-codes`, which modifies the shared registration flow).

## Decisions

### A separate `notebook-wasm` bindings crate

`crates/notebook-wasm` is a `cdylib` depending on `notebook-core` (with its in-memory storage and wasm transport features) plus `wasm-bindgen`, `wasm-bindgen-futures`, `js-sys`, and `serde-wasm-bindgen`. It exports one `#[wasm_bindgen] struct Client` that owns the session (tokens, unwrapped vault key, note documents, indexes, sync engine) and exposes async methods returning Promises: `login`, `register`, `recover`, `change_passphrase`, `reauthenticate`, `logout`, note/tag/search queries, `apply_local_update(note_id, bytes)`, `set_prefs`, and `subscribe(callback)` for events (sync status, pending count, remote note changes, remote updates for open notes, bootstrap progress, re-login required).

*Alternative:* `#[cfg(target_arch = "wasm32")]` bindings inside `notebook-core`. Rejected: it mixes JS types into the platform-independent core, and the Tauri commands already form a parallel thin wrapper, so a sibling crate mirrors `notebook-app`.

Build: `wasm-bindgen-cli` pinned to the exact `wasm-bindgen` version in `Cargo.lock`, run by `web/scripts/build-wasm.sh` (`cargo build -p notebook-wasm --target wasm32-unknown-unknown --release`, then `wasm-bindgen --target web --out-dir web/src/wasm/pkg`). `web/src/wasm/pkg` is git-ignored. `wasm-pack` is not used: it adds a tool with its own version drift for no gain here. The release profile for `notebook-wasm` uses `opt-level = "s"` and `lto = true`; `wasm-opt` is optional and not required by CI.

The workspace `unsafe_code = "deny"` lint still applies; `wasm-bindgen` macro expansion uses `#[allow]` internally and builds under `deny`.

### Data across the boundary

CRDT updates cross as `Uint8Array` (no JSON, no base64). Note summaries, tag trees, search hits, and events cross as plain objects via `serde-wasm-bindgen`, using the same serde types the Tauri commands return, so the TypeScript adapter types are shared between adapters. Errors become a JS `Error` with a stable `code` string (`auth_failed`, `network`, `quota`, `reauth_required`, ...) mapped from core error enums.

### Build-time adapter selection

`desktop-app-shell` defines `web/src/platform/types.ts` (the adapter interface). This change adds `web/src/platform/web/` (WASM implementation) and selects the implementation through a Vite alias `$platform` resolved by mode: `vite build --mode web` → `platform/web`, default → `platform/tauri`. The browser build writes to `web/dist-web/` so Tauri's `web/dist/` is untouched. npm scripts: `build:web` (runs `build-wasm.sh` then `vite build --mode web`) and `dev:web` (Vite dev server with a proxy for `/auth`, `/sync`, and the WebSocket path to a local `notebook-server`, so the page stays same-origin).

*Alternative:* runtime detection with `isTauri()`. Rejected: both adapters and the WASM module would ship in both bundles.

### Core runs on the main thread

The `Client` lives on the main thread. Bootstrap decrypts in batches (e.g. 200 notes) and yields to the event loop between batches, reporting progress. XChaCha20-Poly1305 and Yrs decode are fast enough at personal scale (thousands of notes) that the main thread stays responsive.

*Alternative:* run the core in a dedicated Worker, which mirrors Tauri's message-passing IPC. Deferred: a worker script cannot carry an SRI hash, it doubles the bundle-integrity work, and the editor's per-keystroke update path gets an extra async hop. Revisit if bootstrap of large vaults blocks the UI noticeably.

### Editor binding

The editor uses `note-editor-component`'s document interface. In the browser, local updates call `client.apply_local_update` synchronously within the same tick (encryption and outbox happen in Rust), and remote updates for open notes arrive through the `subscribe` callback and are applied to the editor's document. No browser-specific editor code exists beyond the adapter.

### Session and token handling

- The access token, refresh token, OPAQUE export key, vault key, and note keys live only inside the WASM `Client`. JS never sees keys or tokens; it sees decrypted note text, which it needs for display.
- HTTP calls send the access token as `Authorization: Bearer`. The live WebSocket authenticates without putting the token in the URL (first-message or subprotocol auth, per `sync-server`'s protocol). Cookies are not used by the client.
- The passphrase is read from the input, passed into `login` as a string, and the input is cleared immediately. JS strings cannot be zeroized; the copy inside Rust is held in a `Zeroizing` buffer and dropped after OPAQUE finishes. Inputs use `autocomplete="current-password"`/`"new-password"` so password managers work (user's choice); recovery-key inputs use `autocomplete="off"`.

### Ending the session

- **Sign-out / lock:** call `client.logout()` (best-effort server revocation with a short timeout), then `client.free()` so Rust drops and zeroizes all key material, then reset every Svelte store holding note data and render the sign-in screen.
- **Tab close / reload:** the heap is discarded by the browser. No beacon is sent; the unrevoked server session cannot be used because its refresh token existed only in memory, and it expires on the server's schedule.
- **Back/forward cache:** a `pagehide` listener tears the session down (as sign-out, without waiting for the network), and a `pageshow` listener with `event.persisted` forces `location.reload()`, so a restored page never shows the old vault.
- **Unload warning:** a `beforeunload` handler is attached only while the pending count is above zero and removed when it returns to zero, so synced tabs close silently and stay eligible for bfcache.

### Re-authentication

When the sync engine reports that refresh failed, the adapter shows a passphrase prompt over the current view. `client.reauthenticate(username, passphrase)` runs OPAQUE login, checks the returned vault ID matches the current one, swaps in the new tokens, and resumes sync with the in-memory outbox intact. A different username, or a vault mismatch, results in a full sign-out followed by a normal login.

### Preferences

`web/src/platform/web/prefs.ts` is the only module that touches `localStorage`. It holds a hard-coded allowlist (`notebook.pref.vim`, `notebook.pref.theme`), validates values, wraps every access in try/catch, and falls back to defaults. It backs the adapter's device-local settings; the in-memory `Storage` device-settings table is seeded from it at session start and written through on change. An ESLint `no-restricted-globals`/`no-restricted-properties` rule forbids `localStorage`, `sessionStorage`, `indexedDB`, `caches`, `document.cookie`, and `navigator.serviceWorker` everywhere except `prefs.ts`.

### Markdown export download

`markdown-export` provides `export::plan(notes, ExportOptions { include_trashed }, now)` and `write_zip(&ExportPlan) -> Vec<u8>` in `notebook-core`, and adds `exportNotes({ includeTrashed })` to the platform adapter. `notebook-wasm` exposes `Client::export_zip(include_trashed: bool, now_ms: f64)`, which collects the session's in-memory `Note` values (`NoteDocument::note()`), calls `plan` and `write_zip`, and returns `{ fileName: "<folder_name>.zip", bytes: Uint8Array }`. The time comes from JS (`Date.now()`), matching the core's caller-supplied-time convention.

The browser `exportNotes` wraps the bytes in a `Blob` (`application/zip`), creates an object URL, clicks a temporary `<a download="<fileName>">` that is never attached to persistent UI, removes it, and revokes the URL on the next tick, then drops the byte reference. This is the one deliberate exception to the no-persistence rule: a plaintext file the user explicitly asked for, written by the browser's download manager outside origin storage. The shared settings UI already warns before every export. `blob:` navigation for downloads is not restricted by the server CSP.

### Bundle integrity

A small local Vite plugin (`web/vite/sri.ts`, build-only, `transformIndexHtml` with `order: 'post'`) computes `sha384` over each emitted file referenced by `index.html` and adds `integrity` to its `<script>`, `<link rel="modulepreload">`, and `<link rel="stylesheet">` tags. The browser build disables code splitting so no JS is loaded by a dynamic `import()` that SRI cannot cover. The WASM module's URL and hash go into `<meta name="notebook-wasm" content="/assets/notebook_wasm-<hash>.wasm" data-integrity="sha384-...">`; the loader reads it, calls `fetch(url, { integrity })`, and passes the response to the wasm-bindgen init (`instantiateStreaming`). The hash lives in `index.html` (served `no-cache`) rather than in the JS chunk, because patching a hash into a chunk after Vite named it would break the content-hash-in-filename guarantee that immutable caching relies on.

If the entry script fails SRI, no JS runs, so the failure message is static: `index.html` contains a boot notice whose "failed to load" text is revealed by a CSS `animation-delay` (about 10 seconds) in the external stylesheet; the app removes the notice when it mounts. A WASM integrity or instantiate failure is caught in JS and shows the message immediately.

SRI here protects against tampering in caches or in the static directory after build, not against the server itself (documented).

### CSP compatibility without relaxation

- Svelte component CSS is emitted to a file (`style-src 'self'`). ESLint `svelte/no-inline-styles` forbids `style="..."` attributes in our components; dynamic styling uses classes or the `style:` directive (CSSOM, not governed by CSP).
- CodeMirror's `style-mod` uses constructable stylesheets (`adoptedStyleSheets`), which `style-src` does not govern in supported browsers. `EditorView.cspNonce` is not used (no per-response nonces with static serving).
- `'wasm-unsafe-eval'` already permits WebAssembly compilation. `connect-src 'self'` covers the same-origin WebSocket.
- A Playwright test runs the full flow against the real server and fails on any `securitypolicyviolation` event.

Minimum browsers follow from these: constructable stylesheets, ES2022, and `WebAssembly.instantiateStreaming` (current Chrome, Edge, Firefox, Safari 16.4+).

### Unsupported-browser check

`web/src/platform/web/support.ts` runs before loading WASM and checks `WebAssembly`, `crypto.getRandomValues`, `WebSocket`, and `document.adoptedStyleSheets`. On failure it renders a static message naming the missing capability.

### Testing

- `wasm-bindgen-test` (headless browser via `wasm-bindgen-test-runner`) for the `notebook-wasm` boundary: byte round-trips, error codes, `free()` behavior.
- Vitest unit tests for `prefs.ts`, the SRI plugin, the support check, and the adapter's lifecycle wiring with a fake `Client`.
- Playwright (`web/e2e/`, Chromium in CI, Firefox and WebKit run locally) against a `notebook-server` started on an ephemeral port with a temp database and `static_dir=web/dist-web`. Accounts are created through the browser registration flow. Covers: sign-in/bootstrap, storage audit with a marker string, reload and bfcache, sign-out, two-context live sync and convergence, offline buffering via `context.setOffline`, unload prompt, re-auth after forced token expiry, SRI tamper, CSP violations, unsupported-browser message.

Playwright is the one new dev tool. It is the only practical way to assert browser storage, unload prompts, bfcache, and CSP behavior against the real server.

## Risks / Trade-offs

- [Third-party code sets `style` attributes, causing CSP violations] → The CSP Playwright test catches it. If unavoidable (e.g. a CodeMirror extension using `setAttribute('style')`), prefer replacing the extension; as a last resort add a `server-hosting` delta to this change allowing only `style-src-attr 'unsafe-inline'`, leaving scripts untouched.
- [Malicious or compromised server serves modified JS] → Inherent to browser E2EE; documented in `docs/security.md` with a recommendation to use native apps. SRI does not mitigate this.
- [Main-thread bootstrap stalls on a large vault] → Batched yielding plus progress; Worker or encrypted index snapshot are recorded follow-ups.
- [The exported zip in Downloads is plaintext on the device] → Only on an explicit, warned user action; documented in `docs/security.md` as the one way the browser client leaves data behind.
- [Edits lost if the tab is closed while offline] → Unload warning; the in-memory outbox is by design not persisted.
- [Decrypted text in the JS heap is not zeroizable] → Accepted; it dies with the page. Sign-out drops all references and frees the WASM heap.
- [Cross-change dependency: `sync-server` WebSocket auth must not use a URL token] → Raised to that change; the spec requirement "Secrets kept out of URLs" depends on it.
- [Cross-change dependency: `desktop-app-shell` adapter interface and `note-editor-component` document interface not yet fixed] → This change implements whatever those designs settle; tasks start by conforming to them rather than inventing parallel interfaces.
- [WASM bundle size (Yrs + OPAQUE + crypto)] → `opt-level = "s"`, LTO, immutable caching; measured in a task with a soft budget of 2 MB gzipped.
- [`wasm-bindgen` and `wasm-bindgen-cli` version skew] → Build script checks the CLI version against `Cargo.lock` and fails with a clear message.

## Migration Plan

1. Build with `npm run build:web` and copy `web/dist-web/.` to the server's `static_dir` (`docs/deployment.md` updated from `web/dist`).
2. No server code or schema change. Because `index.html` is `no-cache` and assets are content-hashed, every new tab picks up the new version; open tabs keep the old one until reloaded.
3. Rollback: copy the previous bundle back into `static_dir`.

## Open Questions

- Exact soft budget for the gzipped WASM size and whether to run `wasm-opt` in CI; settle after the first measured build.
- Whether to run Playwright's Firefox and WebKit projects in CI as well as Chromium; depends on CI time once the suite exists.
