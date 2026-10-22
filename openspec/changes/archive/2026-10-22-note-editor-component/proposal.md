# Proposal

## Why

Every client needs one markdown editor that supports optional vim bindings and edits the note's CRDT document directly, so concurrent edits from other devices merge live into the open note. Building it as a shared frontend component, used by the Tauri webview and the browser, avoids maintaining two editors. This change delivers a functional editor. Making it beautiful is a separate follow-up (`editor-live-preview`).

## What Changes

- A CodeMirror 6 editor component in the shared Svelte/TypeScript frontend (`web/`):
  - Markdown language support with syntax highlighting, including Bear-style tag highlighting that matches the core tag parser rules (checked against a fixture shared with the Rust tests).
  - Collaborative binding (`y-codemirror.next`) to an in-editor replica of the note's CRDT document, so local edits produce updates and remote updates appear in place with the cursor position preserved.
  - Standard editing affordances: undo/redo scoped to local edits, find/replace, soft wrap, and list continuation on Enter.
- **Optional vim mode** (`@replit/codemirror-vim`):
  - Toggled at runtime without reloading the editor (CM6 compartment).
  - A **per-device** setting, persisted through the platform's device-local settings, defaulting to off.
  - `:w` flushes pending edits to the core and acknowledges, since saving is otherwise automatic.
- A narrow integration interface (the note document bridge: open a note's document, push local updates, receive remote updates) so the editor doesn't depend on whether the core runs over Tauri IPC or WASM. This change ships the TypeScript contract plus an in-memory reference implementation for tests; the Tauri and WASM implementations belong to `desktop-app-shell` and `browser-web-client`.
- In `notebook-core`, a deliberate **editor exchange** on note documents: the core keeps owning the authoritative document, its key, and encryption, and exchanges plaintext Yjs-format updates only with the open editor replica. Every editor update the core accepts is turned into an encrypted update for storage and sync.

## Capabilities

### New Capabilities
- `note-editor`: Markdown note editing behavior: syntax and tag highlighting, live merging of concurrent edits, local undo/redo, find/replace, soft wrap, list continuation, durability of typed edits, and the optional, per-device vim mode.

### Modified Capabilities
- `note-documents` (introduced by `crdt-note-documents`): adds the editor exchange, a plaintext update path between a note document and the open editor replica that never reaches storage or sync.

## Impact

- Frontend (`web/`): new `src/editor/` module; dependencies `@codemirror/state`, `@codemirror/view`, `@codemirror/commands`, `@codemirror/language`, `@codemirror/lang-markdown`, `@codemirror/search`, `@replit/codemirror-vim`, `yjs`, `y-codemirror.next`; `jsdom` for component tests.
- `notebook-core`: editor exchange methods on `NoteDocument` (from `crdt-note-documents`) and editor entry points on `sync-client-engine`'s `Replica`, which hosts call so every editor edit gets the replica's single storage transaction and outbox entry; plus a tag-parser fixture file shared with the frontend tests. Both upstream changes must land first.
- Consumed by `desktop-app-shell`, `browser-web-client`, and `mobile-app`, which implement the bridge and the device-settings store for their platform. Extended by `editor-live-preview`.
- CSP: the editor must run under the server's strict `style-src 'self'` policy as well as the Tauri policy.
