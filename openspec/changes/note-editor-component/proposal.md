# Proposal

## Why

Every client needs one markdown editor that supports optional vim bindings and edits the note's CRDT document directly, so concurrent edits from other devices merge live into the open note. Building it as a shared frontend component, used by the Tauri webview and the browser, avoids maintaining two editors. This change delivers a functional editor. Making it beautiful is a separate follow-up (`editor-live-preview`).

## What Changes

- A CodeMirror 6 editor component in the shared TypeScript frontend package:
  - Markdown language support with syntax highlighting, including Bear-style tag highlighting consistent with the core tag parser rules.
  - Collaborative binding (`y-codemirror.next`) to the note's CRDT document, so local edits produce updates and remote updates appear in place with cursor position preserved.
  - Standard editing affordances: undo/redo scoped to local edits, find/replace, soft wrap, and list continuation on Enter.
- **Optional vim mode** (`@replit/codemirror-vim`):
  - Toggled at runtime without reloading the editor (CM6 compartment).
  - A **per-device** setting, persisted through the platform's device-local settings, defaulting to off.
  - `:w` is a no-op or acknowledgment, since saving is automatic.
- A narrow integration interface (load a note document, emit updates, receive remote updates) so the editor doesn't depend on whether the core runs over Tauri IPC or WASM.

## Capabilities

### New Capabilities
- `note-editor`: Markdown note editing behavior: syntax and tag highlighting, live merging of concurrent edits, local undo/redo, find/replace, and the optional, per-device vim mode.

### Modified Capabilities
<!-- None. -->

## Impact

- Frontend package: `@codemirror/*`, `@replit/codemirror-vim`, `yjs`, `y-codemirror.next`.
- Bridge to the core: Yjs in JS and Yrs in Rust share a wire format. The design must settle which side owns the live document and how updates cross the boundary.
- Consumed by `desktop-app-shell`, `browser-web-client`, and `mobile-app`.
