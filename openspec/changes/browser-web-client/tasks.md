# Tasks

Prerequisites: `crdt-note-documents`, `local-sqlite-storage`, `opaque-accounts`, `sync-server`, `sync-client-engine`, `note-editor-component`, `desktop-app-shell`, and `markdown-export` are archived, so the core session, in-memory storage, export `plan`/`write_zip`, wasm transport, platform adapter interface, and editor document interface exist.

## 1. WASM bindings crate and build

- [ ] 1.1 Create `crates/notebook-wasm` (`cdylib` + `rlib`) depending on `notebook-core` with its in-memory storage and wasm transport features, plus `wasm-bindgen`, `wasm-bindgen-futures`, `js-sys`, and `serde-wasm-bindgen` added to the workspace; add a `[profile.release.package.notebook-wasm]` with `opt-level = "s"` and enable LTO for the release wasm build; verify `cargo build -p notebook-wasm --target wasm32-unknown-unknown --release` succeeds and `cargo clippy --workspace --all-targets -- -D warnings` stays clean
- [ ] 1.2 Write `web/scripts/build-wasm.sh` that checks the installed `wasm-bindgen-cli` version against `Cargo.lock` (failing with an install hint on mismatch), builds the crate, and runs `wasm-bindgen --target web --out-dir web/src/wasm/pkg`; git-ignore `web/src/wasm/pkg` and `web/dist-web`; verify running the script produces `notebook_wasm.js`, `notebook_wasm_bg.wasm`, and `.d.ts` files, and that a mismatched CLI version fails with the hint
- [ ] 1.3 Add `build-wasm.sh` and the wasm build to `scripts/check.sh` and the CI `wasm` job (installing the pinned `wasm-bindgen-cli`); verify `scripts/check.sh` exits 0 locally

## 2. Client bindings

- [ ] 2.1 Implement the `Client` export: constructor taking the server origin, `subscribe(callback)` for events (sync status, pending count, bootstrap progress, remote note changes, remote updates for open notes, `reauth_required`), JS `Error` mapping with stable `code` values, and `free()` dropping all key material; verify `wasm-bindgen-test` tests in a headless browser for error codes, event delivery, and that calls after `free()` fail cleanly
- [ ] 2.2 Implement account methods (`login` with batched bootstrap and progress events, `register`, `recover`, `change_passphrase`, `logout` with a short timeout that never blocks local teardown) holding the passphrase in `Zeroizing` buffers; verify `wasm-bindgen-test` tests against a mocked transport for successful login and bootstrap progress, generic failure on a wrong passphrase, and logout succeeding locally when the network fails
- [ ] 2.3 Implement `reauthenticate(username, passphrase)` that keeps the outbox, swaps tokens, and resumes sync only when the username and vault ID match, otherwise returning a `session_mismatch` error; verify `wasm-bindgen-test` tests for the matching case (outbox preserved) and both mismatch cases
- [ ] 2.4 Implement note, tag, search, and document methods using the same serde types as the Tauri commands, with CRDT updates as `Uint8Array` (`open_note` returning the encoded state, `apply_local_update`, note lifecycle and tag rename/move); verify `wasm-bindgen-test` byte round-trip tests and a test that a local update emits an encrypted outbox entry

- [ ] 2.5 Implement `export_zip(include_trashed, now_ms)` on `Client`, building `ExportOptions` from the flag, calling `export::plan` over the session's in-memory notes with `Timestamp(now_ms)` and then `write_zip`, and returning `{ fileName, bytes: Uint8Array }` with `fileName = <folder_name>.zip`; verify a `wasm-bindgen-test` that a session with an archived and a trashed note returns a zip whose entries (read back with the `zip` reader) match `plan` for both flag values and whose file name matches the plan's folder name
- [ ] 2.6 Supply the `reopen` storage callback `sync-client-engine` needs for its server-restore resync, returning a fresh in-memory store (`MemoryStorage::new`) and nothing persisted, and emit a remote-change event for every note afterwards so the shared stores re-query; verify a `wasm-bindgen-test` against a mocked transport answering `cursor_ahead` that the session's notes survive the resync and their full updates are pushed

## 3. Browser platform adapter

- [ ] 3.1 Add the `$platform` Vite alias resolving to `platform/web` under `--mode web` and `platform/tauri` otherwise, output `web/dist-web/` for web mode with code splitting disabled, and npm scripts `build:web` and `dev:web` (proxying auth, sync, and the WebSocket path to a local server); verify `npm run build` output contains no WASM and `npm run build:web` output contains no `@tauri-apps` code (grep both outputs)
- [ ] 3.2 Implement `platform/web/support.ts` (checks `WebAssembly`, `crypto.getRandomValues`, `WebSocket`, `adoptedStyleSheets`, rendering a message naming the missing capability) and the WASM loader reading `<meta name="notebook-wasm">` and fetching with `integrity`, showing a load-failure message on error; verify Vitest tests for each missing capability and for a rejected integrity fetch
- [ ] 3.3 Implement `platform/web/prefs.ts` with the `notebook.pref.vim` and `notebook.pref.theme` allowlist, value validation, try/catch fallback to defaults, and write-through from device-local settings; add ESLint `no-restricted-globals`/`no-restricted-properties` rules banning `localStorage`, `sessionStorage`, `indexedDB`, `caches`, `document.cookie`, and `navigator.serviceWorker` outside `prefs.ts`, and enable `svelte/no-inline-styles`; verify Vitest tests for allowlist enforcement, invalid values, and a throwing `localStorage`, and that `npm run lint` fails on a scratch file using `localStorage`
- [ ] 3.4 Implement the WASM adapter conforming to the `desktop-app-shell` adapter interface and the `note-editor-component` document interface, mapping `Client` events to the shared stores; verify Vitest tests with a fake `Client` that remote updates reach the editor binding and status events update the sync indicator store
- [ ] 3.5 Implement session lifecycle wiring: sign-out and lock (logout, `free()`, reset every note-holding store, show sign-in), `pagehide` teardown, `pageshow` with `persisted` forcing reload, and a `beforeunload` handler attached only while the pending count is above zero; verify Vitest tests with a fake `Client` that lock equals sign-out, stores are empty after sign-out, and the `beforeunload` listener is present only while changes are pending
- [ ] 3.6 Implement the re-authentication prompt shown on `reauth_required`, calling `reauthenticate` and falling back to full sign-out and fresh login on `session_mismatch`; verify Vitest tests for both paths with a fake `Client`
- [ ] 3.7 Add the browser-only sign-in notice ("nothing is stored on this device; closing the tab signs you out") and wire registration, recovery, and passphrase change to the shared `app-shell` flows; verify a Vitest component test renders the notice in web mode only
- [ ] 3.8 Implement the browser `exportNotes({ includeTrashed })`, replacing the `markdown-export` stub: call `client.export_zip(includeTrashed, Date.now())`, wrap the bytes in an `application/zip` `Blob`, click a temporary detached `<a download>` with the returned file name, revoke the object URL on the next tick, and return the shared `ExportResult`; verify Vitest tests with a fake `Client` that the anchor gets the right `download` name and `blob:` href, that `URL.revokeObjectURL` is called, and that no storage API is touched

## 4. Bundle integrity

- [ ] 4.1 Implement the build-only Vite plugin `web/vite/sri.ts` adding `sha384` integrity to `index.html` script, modulepreload, and stylesheet tags and emitting the `notebook-wasm` meta tag with the hashed WASM URL and its integrity; verify Vitest tests on a fixture bundle and that every tag in a real `dist-web/index.html` has an integrity value matching `openssl dgst -sha384 -binary | base64` of its file
- [ ] 4.2 Add the static boot notice to `index.html` with a CSS `animation-delay` reveal of the failure text, removed when the app mounts; verify `npm run build:web` keeps the notice in `index.html` with no inline script or style attribute

## 5. End-to-end tests

- [ ] 5.1 Add Playwright as a dev dependency with `web/e2e/` and a fixture that builds `dist-web`, starts `notebook-server serve` on an ephemeral port with a temp database and `static_dir` pointed at it, and tears it down; add `npm run e2e`; verify a smoke test loads `/` and sees the sign-in screen with the notice
- [ ] 5.2 Write e2e tests for registration then sign-in with bootstrap, wrong passphrase, and the storage audit (marker string typed into a note, then IndexedDB, OPFS, Cache Storage, service workers, cookies, and local/session storage inspected); verify they pass on Chromium
- [ ] 5.3 Write e2e tests for reload and back/forward-cache restore showing sign-in, sign-out and lock clearing the view, vim mode persisting across sessions, and two independent tabs; verify they pass on Chromium
- [ ] 5.4 Write e2e tests with two browser contexts for live sync and concurrent-edit convergence, offline buffering with `context.setOffline` and flush on reconnect, the unload prompt only while pending, and re-authentication after server-side session revocation keeping buffered edits; verify they pass on Chromium
- [ ] 5.5 Write an e2e test that chooses "Export all notes" after typing a marker string into a note, captures the download, and asserts a single `.zip` whose extracted contents include the marker note, then re-runs the storage audit and asserts origin storage is unchanged; verify it passes on Chromium
- [ ] 5.6 Write e2e tests that fail on any `securitypolicyviolation` during a full session with vim mode and search, that a tampered WASM file in `static_dir` shows the load-failure message, that no request URL (including the WebSocket) carries a credential, and that disabling WebAssembly shows the unsupported message; verify they pass on Chromium and fix any CSP violation without relaxing the server policy (see design Risks)
- [ ] 5.7 Add an `e2e` CI job (Chromium) after the `web` and `rust` jobs; verify it passes on a pushed branch

## 6. Documentation

- [ ] 6.1 Write `docs/security.md` covering the browser trust model (server delivers code on every load, what SRI does and does not cover, nothing stored on device except a markdown export the user explicitly downloads, recommendation to use native apps when that trust is unacceptable) and update `docs/deployment.md` to build with `npm run build:web` and copy `web/dist-web/`; verify following the updated deployment steps against a local server serves a working client
- [ ] 6.2 Update the README with how to build and run the browser client locally (`build-wasm.sh`, `dev:web`, `e2e`); verify the commands run as written from a clean checkout

## 7. Integration checks

- [ ] 7.1 Manually edit the same note in the desktop app and the browser against one server and confirm edits appear both ways within seconds, and that browser edits made offline arrive on desktop after reconnecting
- [ ] 7.2 Record the gzipped WASM size against the design's soft budget and note the figure in the design's Open Questions resolution
- [ ] 7.3 Run `scripts/check.sh` and `npm run e2e`; verify both exit 0
- [ ] 7.4 Run `openspec validate browser-web-client --strict` and verify it passes

## Workflow follow-up

- Archive the change after review, then mark item 13 done in `TODO.md`.
