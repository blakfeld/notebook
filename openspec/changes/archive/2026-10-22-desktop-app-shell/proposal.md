# Proposal

## Why

With the core, storage, sync, and editor in place, users need an actual application: sign in, see their tag tree, browse and open notes, and edit them offline with changes syncing everywhere. The Tauri desktop app (macOS, Windows, Linux) is the first full client and defines the app layout that the browser and mobile clients reuse.

## What Changes

- Tauri 2 desktop app wiring `notebook-core` (sync engine plus SQLite storage) to the shared frontend through Tauri commands and events.
- **Account flows**: sign in, register with an invite code (showing the recovery key once, with explicit acknowledgment), recover with a recovery key, change passphrase, sign out.
- **Server address**: the user chooses the server at sign-in (HTTPS required, plain HTTP only for loopback during development).
- **Credential handling**: the refresh token and vault key material are kept in the OS keychain, so the app unlocks on launch without retyping the passphrase. An explicit "lock" action clears in-memory keys and requires the passphrase to unlock, which also works offline. With no OS secure store available, nothing is persisted and the user signs in each launch.
- **Three-pane layout**:
  - Sidebar: nested tag tree (expand/collapse) plus Notes, Untagged, Pinned, Archive, Trash.
  - Note list: notes for the selected tag or view, sorted by last updated, showing title and preview.
  - Editor: the `note-editor` component.
- **Note operations**: create, pin/unpin, archive, trash, restore, delete permanently.
- **Tag operations**: rename or move a tag from the sidebar, rewriting every affected note.
- **Search**: local full-text search across decrypted notes, scoped to the selected sidebar item. The index lives only in memory.
- **Sync indicator**: synced/syncing/offline plus pending count.
- **Settings**: vim mode toggle (per device) and account/session info.

## Capabilities

### New Capabilities
- `app-shell`: The client application experience: account and unlock flows, sidebar tag tree and special views, note list, note lifecycle actions, tag rename/move, local search, sync status, and settings.

### Modified Capabilities
<!-- None. -->

## Impact

- `notebook-core`: a platform-neutral `workspace` module (the unlocked vault: note queries, views, note and tag operations) and an in-memory `search` module, both wasm-compatible so `browser-web-client` reuses them.
- `notebook-app` crate: Tauri commands and events, app state machine (signed out / locked / unlocked), keychain credential store, SQLite storage and sync engine wiring.
- Frontend (`web/`): a platform adapter interface with a Tauri implementation, app stores, auth screens, and the three-pane layout hosting the `note-editor` component.
- Dependencies: `keyring` (OS keychain: macOS Keychain, Windows Credential Manager, Linux Secret Service). No search library: an in-memory index avoids writing plaintext to disk and builds for wasm.
- Builds on `accounts` (`opaque-accounts`), `sync-protocol` (`sync-server`), `sync-client` (`sync-client-engine`), `local-storage` (`local-sqlite-storage`), `note-documents` (`crdt-note-documents`), and `note-editor` (`note-editor-component`). Registration needs the server to accept invite codes (`invite-codes`) before it works against a production server.
- `app-shell` requirements are written platform-neutrally so `browser-web-client` and `mobile-app` can reuse or adjust them. Requirements about the OS secure store apply to native clients only.
