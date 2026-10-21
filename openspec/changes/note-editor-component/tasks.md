# Tasks

Prerequisites: `crdt-note-documents` is implemented (`NoteDocument`, `EncryptedUpdate`, `DocumentError`), and `sync-client-engine`'s `Replica` exists in `notebook-core::sync` with its local-edit transaction path.

## 1. Core editor exchange (`notebook-core`)

- [ ] 1.1 Add `EditorUpdate` (plaintext Yjs v1 bytes; `from_bytes`/`as_bytes`/`into_bytes`; redacted `Debug`; no conversions to or from the encrypted types) and `EditorSync` (wraps a state vector) to the `document` module, re-exported from the crate root; verify `cargo build -p notebook-core` and the wasm32 build succeed and `tests/public_api.rs` covers the new names
- [ ] 1.2 Implement `NoteDocument::editor_attach` returning the full state as an `EditorUpdate` plus an `EditorSync` at the current state vector; verify a test that starts a `yrs::Doc` from it and gets the same body
- [ ] 1.3 Implement `apply_editor_update(&mut self, &mut EditorSync, &EditorUpdate, now)`: decode before any transaction (`Malformed` on failure, state unchanged), apply, set `meta.updated = now` only if the body changed, return one encrypted `CrdtUpdate` for the before/after diff or `None` if nothing changed, and merge the update's state vector into the tracker; verify tests for the spec scenarios "Typed text is encrypted for sync", "Duplicate editor update", and "Garbage editor update"
- [ ] 1.4 Implement `editor_pending(&self, &mut EditorSync) -> Option<EditorUpdate>` (diff from the tracker, then advance it); verify tests for "Remote change reaches the editor", "Nothing new", and "Updated time reaches the editor"
- [ ] 1.5 Add a randomized convergence test: a core document and a separate `yrs::Doc` acting as the editor make interleaved edits, exchange through `apply_editor_update`/`editor_pending` while a third replica sends encrypted updates, and all three end with identical body and metadata; verify it passes over at least 200 seeded runs with `cargo test -p notebook-core`

- [ ] 1.6 Add `EditorSessionId` and the `Replica` editor entry points `editor_attach`, `apply_editor_update` (calls `NoteDocument::apply_editor_update`, then stores the encrypted update and adds the outbox entry in one storage transaction through the replica's existing local-edit path, then wakes the engine), `editor_pending`, and `editor_detach`, with trackers held inside the replica; verify replica tests with in-memory storage that an editor update produces exactly one stored update and one outbox entry, a no-op update produces neither, a storage failure leaves memory matching storage and a resend succeeds, a remote batch via `apply_remote` is returned by `editor_pending`, and unknown/detached sessions and purged notes are errors

## 2. Shared tag fixture

- [ ] 2.1 Create `crates/notebook-core/tests/fixtures/tag_cases.json` (`[{ "body", "tags": [occurrence source text, …] }]`) seeded with every scenario body in `openspec/specs/tags/spec.md` plus headings, URL fragments, code spans/blocks, and non-ASCII cases, and a Rust test asserting `tags::parse` yields exactly those texts in order; verify `cargo test -p notebook-core` passes

## 3. Frontend setup

- [ ] 3.1 Add `@codemirror/state`, `@codemirror/view`, `@codemirror/commands`, `@codemirror/language`, `@codemirror/lang-markdown`, `@codemirror/search`, `@replit/codemirror-vim`, `yjs`, `y-codemirror.next` as dependencies and `jsdom` as a dev dependency in `web/`; verify `npm ls @codemirror/state yjs` shows a single deduplicated version of each and `npm run build` succeeds
- [ ] 3.2 Configure Vitest so `src/editor/**/*.test.ts` runs under `jsdom` (per-file `@vitest-environment jsdom` or `environmentMatchGlobs`) and existing tests still run in node; verify `npm test` passes

## 4. Bridge contract and reference bridge

- [ ] 4.1 Write `web/src/editor/bridge.ts` with `NoteDocBridge` and `OpenNoteDoc` as in design.md, with doc comments stating the byte format (Yjs v1, no format byte), idempotency, delivery timing, the `editor.vimMode` key, and that platforms implement it with `editor_attach`/`apply_editor_update`/`editor_pending`; verify `npm run check` passes
- [ ] 4.2 Implement `web/src/editor/memoryBridge.ts` (a `Y.Doc` per note standing in for the core, `remoteEdit(noteId, fn)` to simulate another device, optional failure injection for `push`); verify unit tests that open, push, and receive a remote edit converge, and that the bridge reads and writes only the `body` root
- [ ] 4.3 Write `web/src/platform/settings.ts` with the `DeviceSettings` interface (doc comments: device-local, non-sensitive, never synced, string keys and values, missing or failed read means default) and `memorySettings()` with optional read/write failure injection; verify `npm run check` passes and unit tests cover get/set, missing keys, and failures

## 5. Editor core

- [ ] 5.1 Implement `createNoteEditor(parent, { bridge, settings, onError })` in `web/src/editor/noteEditor.ts` with markdown language and highlighting, `yCollab` + `Y.UndoManager` (no CodeMirror `history()`), `yUndoManagerKeymap`, `search` + `searchKeymap`, `lineWrapping`, `markdownKeymap`, `defaultKeymap`, `indentWithTab`, and the `NoteEditorHandle` methods; verify jsdom tests for "Opening a note shows its body" and "Typing changes the document"
- [ ] 5.2 Implement remote application with a `REMOTE` origin excluded from pushes and undo; verify tests for "Remote edit appears", "Concurrent local and remote edits", "Remote insert before the cursor", "Remote edit after the selection", "Undo skips remote edits", "Redo", and "Fresh history on open"
- [ ] 5.3 Implement coalesced delivery (merge with `Y.mergeUpdates`, 300 ms idle, 1 s cap, `flush()`, flush on `openNote`, `destroy`, `visibilitychange` hidden, and `pagehide`) and failure handling (keep pending, call `onError`, retry); verify tests with fake timers for "Idle delivery", "Closing delivers immediately", "Switching notes keeps pending edits", and "Delivery fails then succeeds"
- [ ] 5.4 Verify list continuation and find/replace through the installed commands: tests for the four list-continuation scenarios and "Replace all" (including that one undo restores the original) pass

## 6. Tag highlighting

- [ ] 6.1 Implement `findTagRanges` in `web/src/editor/tags.ts` as a port of the Rust scanner rules (boundary before `#`, single-word, closed, multi-word, trailing punctuation, numeric-only rejection, empty segments) and a `ViewPlugin` that marks visible-line matches outside `InlineCode`/`FencedCode`/`CodeBlock` nodes with `cm-tag`; verify a Vitest test loads `crates/notebook-core/tests/fixtures/tag_cases.json` and the editor's `cm-tag` spans equal each case's `tags`, plus tests for "Single-word and multi-word tags", "Non-tags are not highlighted", and "Highlighting follows edits"

## 7. Vim mode

- [ ] 7.1 Add the vim `Compartment` (vim extension highest precedence), `setVimMode(on)`, and map vim `u`/`Ctrl-r` to the Yjs undo manager; verify tests for "Vim off by default", "Vim motions when on", and "Toggle while editing" (body, cursor, and undo preserved), and that `u` in normal mode undoes a local edit but not a remote one
- [ ] 7.2 Read and write `editor.vimMode` through `DeviceSettings` (absent means off; `setVimMode` persists); verify tests with `memorySettings()` for "Persists on the device" (a new editor on the same settings starts in vim mode), "Unreadable preference", "Failed save", and that the setting never goes through the bridge
- [ ] 7.3 Define ex commands: `:w` flushes and shows "Saved"; `:wq`, `:x`, `:q` flush and do nothing else; verify tests for "Write" and "Quit does nothing destructive"

- [ ] 7.4 Add `setReadOnly(on)` (a compartment with `EditorState.readOnly` and `EditorView.editable`); verify tests for "Read-only note" and "Remote edits still appear while read-only", including that vim `dd` and replace-all change nothing

## 8. Svelte component and placeholder page

- [ ] 8.1 Write `web/src/editor/NoteEditor.svelte` (props `bridge`, `settings`, `noteId`, `vimMode`, `readOnly`, `onError`; creates the editor on mount, calls `openNote` when `noteId` changes, `setVimMode`/`setReadOnly` when those change, `destroy` on unmount) and `web/src/editor/index.ts` exporting the component, handle type, bridge types, and memory bridge; verify `npm run check` and `npm run lint` pass and a component test mounts it, switches notes, and toggles vim
- [ ] 8.2 Mount `NoteEditor` in the placeholder `App.svelte` on the memory bridge with a sample note (headings, lists, tags, code) and a vim checkbox, plus a button that applies a simulated remote edit; verify with `npm run dev` that typing, wrapping, find/replace, tag highlighting, vim toggle, and the remote edit work in a browser with no console errors

## 9. Integration checks

- [ ] 9.1 Build the web bundle and serve it with `notebook-server`'s static hosting and CSP (`style-src 'self'`); verify in Chromium, Firefox, and Safari/WebKit that the editor's styles and tag highlighting apply and the "Strict policy" scenario holds, and record any blocked inline style attributes (moving any that matter into `editor.css`)
- [ ] 9.2 Run `cargo tauri dev` and verify the placeholder editor works in the Tauri webview on macOS (typing, vim toggle, find/replace)
- [ ] 9.3 Run `scripts/check.sh`; verify it exits 0

## Workflow follow-up

- Review and archive after `crdt-note-documents` has been archived, so the `note-documents` additions apply on top of its main spec; when archiving, narrow its "never emit plaintext updates" wording to storage and sync (see design.md Open Questions).
- `desktop-app-shell` and `browser-web-client` implement `NoteDocBridge` (over `Workspace` → `Replica` editor entry points and the change-notification stream) and `DeviceSettings` for their platforms, and replace the placeholder page.
