# Proposal

## Why

Every client (desktop, mobile, browser) needs the same logic to push local edits, pull remote ones, survive disconnections, and keep the op log compact. Implementing the sync engine once in `notebook-core` ensures native and browser clients converge identically, and lets the UI layers stay thin.

## What Changes

- A sync engine in `notebook-core`, parameterized over `Storage` and a transport abstraction (HTTP plus WebSocket; native and wasm implementations):
  - **Outbox**: local CRDT updates are encrypted, persisted to the outbox, and pushed. They're removed only when the server acknowledges them.
  - **Pull and apply**: fetch updates after the stored cursor, decrypt, apply to the note documents, and advance the cursor.
  - **Bootstrap**: on a fresh client, load snapshots and updates for the whole vault.
  - **Live mode**: subscribe over WebSocket and apply incoming updates immediately. Fall back to polling and reconnect with backoff.
  - **Snapshotting**: when a note's update count since its last snapshot passes a threshold, produce and upload an encrypted snapshot.
- Expose sync status to the UI: synced, syncing, offline, and a pending-changes count.
- Notify the UI of remotely changed notes so open editors and the tag tree update live.
- Handle auth expiry by refreshing the session, and surface a re-login requirement when refresh fails.

## Capabilities

### New Capabilities
- `sync-client`: Client-side sync behavior: outbox durability, push/pull, bootstrap, live updates with reconnection, client-driven snapshotting, sync status reporting, and change notifications.

### Modified Capabilities
<!-- None. -->

## Impact

- `notebook-core`: `sync` module, depending on `note-documents`, `local-storage`, `vault-crypto`, and the `sync-protocol` types.
- Transport implementations: `reqwest`/`tokio-tungstenite` (native) and `web-sys`/`gloo` (wasm). Needs an async runtime abstraction that works on both.
- Consumed by the desktop app, browser client, and mobile app.
