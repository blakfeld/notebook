# Proposal

## Why

Every client (desktop, mobile, browser) needs the same logic to push local edits, pull remote ones, survive disconnections, and keep the op log compact. Implementing the sync engine once in `notebook-core` ensures native and browser clients converge identically, and lets the UI layers stay thin.

## What Changes

- A sync engine in `notebook-core`, parameterized over `Storage` and a transport abstraction (HTTP plus WebSocket; native and wasm implementations):
  - **Local replica**: the unlocked vault's note documents, loaded from storage. Local edits are encrypted, written to storage and the outbox in one step, then pushed. New notes get a fresh note key, wrapped by the vault key, that travels with the note.
  - **Outbox**: queued updates are removed only when the server acknowledges them (or echoes them back). Retries reuse the same client update ID, so the server never stores an update twice.
  - **Pull and apply**: fetch updates after the stored cursor, decrypt, apply to the note documents, and advance the cursor only once they are stored.
  - **Bootstrap**: on a fresh client, load snapshots and updates for the whole vault.
  - **Live mode**: subscribe over WebSocket and apply incoming updates immediately. Fall back to polling and reconnect with backoff.
  - **Snapshotting**: when a note's update count since its last snapshot passes a threshold, produce and upload an encrypted snapshot, then compact local storage the same way.
  - **Permanent purge**: purge a trashed note while online, and remove notes that other devices purged.
- Bad remote input (tampered ciphertext, an unknown document format) is contained to the affected note and never stops sync for the rest of the vault.
- Expose sync status to the UI: synced, syncing, offline, and a pending-changes count, plus states that need the user (re-login, client upgrade, storage quota full).
- Recover from a server restored from backup by bootstrapping again and re-sending this client's state of every note.
- Notify the UI of remotely changed notes so open editors and the tag tree update live.
- Handle auth expiry by refreshing the session, and surface a re-login requirement when refresh fails. Pending changes are kept.
- A flush operation so hosts can push pending changes before backgrounding or closing.

## Capabilities

### New Capabilities
- `sync-client`: Client-side sync behavior: local edit durability and the outbox, push/pull, bootstrap, live updates with reconnection, client-driven snapshotting, purge propagation, handling of bad remote input, sync status reporting, and change notifications.

### Modified Capabilities
<!-- None. Consumes `note-documents`, `local-storage`, `vault-crypto`, `accounts` sessions, and the `sync-protocol` contract without changing their requirements. -->

## Impact

- `notebook-core`: `sync` module, depending on `note-documents`, `local-storage`, `vault-crypto`, and the `sync-protocol` types (introduced by `sync-server`).
- Transport implementations behind cargo features: `reqwest`/`tokio-tungstenite` (native) and `web-sys`/`gloo` (wasm). The engine runs as one task without `Send` bounds, and takes timers from the host, so it works on both.
- Uses the `sync-protocol` v1 endpoints from `sync-server` as designed there: wrapped keys ride on a note's first push, pulls return snapshots across compaction and purge tombstones, and purge is an online `DELETE`, pushes go over the live socket when it is up and over HTTP otherwise, and `cursor_ahead` triggers a resync. Echo recognition and key-upload state are owned by `local-storage`.
- Consumed by the desktop app, browser client, and mobile app.
