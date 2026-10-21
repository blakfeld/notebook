# Proposal

## Why

With the core, storage, sync, and editor in place, users need an actual application: sign in, see their tag tree, browse and open notes, and edit them offline with changes syncing everywhere. The Tauri desktop app (macOS, Windows, Linux) is the first full client and defines the app layout that the browser and mobile clients reuse.

## What Changes

- Tauri 2 desktop app wiring `notebook-core` (sync engine plus SQLite storage) to the shared frontend through Tauri commands and events.
- **Account flows**: sign in, register with an invite code (showing the recovery key once, with explicit acknowledgment), recover with a recovery key, change passphrase, sign out.
- **Credential handling**: refresh token and wrapped key material are kept in the OS keychain, so the app unlocks on launch without retyping the passphrase. An explicit "lock" action clears in-memory keys.
- **Three-pane layout**:
  - Sidebar: nested tag tree (expand/collapse) plus Notes, Untagged, Pinned, Archive, Trash.
  - Note list: notes for the selected tag or view, sorted by last updated, showing title and preview.
  - Editor: the `note-editor` component.
- **Note operations**: create, pin/unpin, archive, trash, restore, delete permanently.
- **Tag operations**: rename or move a tag from the sidebar, rewriting every affected note.
- **Search**: local full-text search across decrypted notes.
- **Sync indicator**: synced/syncing/offline plus pending count.
- **Settings**: vim mode toggle (per device) and account/session info.

## Capabilities

### New Capabilities
- `app-shell`: The client application experience: account and unlock flows, sidebar tag tree and special views, note list, note lifecycle actions, tag rename/move, local search, sync status, and settings.

### Modified Capabilities
<!-- None. -->

## Impact

- `notebook-app` crate (Tauri commands and events) and the frontend package (layout, routing, stores).
- Dependencies: Tauri 2 plugins for keychain/secure storage, and possibly a search library (simple in-memory index or `tantivy`).
- `app-shell` requirements are written platform-neutrally so `browser-web-client` and `mobile-app` can reuse or adjust them.
