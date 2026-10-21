# Proposal

## Why

Editing the same note from the native app and the browser requires the server to relay encrypted CRDT updates between a user's clients. Because the server can't read or merge content, it acts as a durable, ordered, encrypted mailbox. This change defines that protocol and the server's side of it.

## What Changes

- An append-only, per-vault **operation log**: clients push encrypted CRDT updates tagged with note ID. The server assigns a strictly increasing per-vault sequence number and stores them unchanged.
- A per-vault **note registry**: the first push for a note carries the note key wrapped by the vault key. The server stores it write-once and hands it to every client that pulls the note, since a client can't decrypt a note created on another device without it.
- **Pull** (`after` a cursor): returns every change after the client's cursor, paginated. Where an encrypted snapshot covers updates the server has already compacted away, the snapshot is returned in their place, along with the wrapped keys of notes the client hasn't seen.
- **Bootstrap** is a pull from cursor zero: the latest snapshot of every note plus the updates after it. Used by the thin browser client on login and by a fresh native install.
- **Live channel** (WebSocket): an authenticated client subscribes to its vault and receives each newly appended update and purge as it happens. Pushes may also go over the socket, with the same semantics as HTTP push; a rejected push gets a per-push error frame carrying an error code and the socket stays open.
- **Snapshots and compaction**: a client can upload an encrypted snapshot declaring "covers note N through sequence S". The server keeps only the latest snapshot per note and deletes that note's updates at or below S. The server never creates snapshots itself.
- **Idempotency**: each pushed update carries a client-generated ID, so a retried push returns the original sequence numbers instead of storing duplicates.
- Note deletion propagates as an encrypted update (the CRDT `trashed` flag). **Permanent purge** of a note removes its updates, snapshot, and wrapped key server-side, leaves a tombstone that pulls and the live channel report, and refuses later pushes for that note.
- Enforce authentication, per-vault isolation, and per-update and per-snapshot size limits on every write. Each write path (push over HTTP or WebSocket, snapshot upload, purge) is a single transaction with a known net byte delta, so `invite-codes` can add its per-vault quota check inside it. Quota accounting and enforcement are owned by `invite-codes`, not this change.

## Capabilities

### New Capabilities
- `sync-protocol`: The client-server sync contract: encrypted op log, sequence ordering, note-key registry, paginated pull and bootstrap, live updates, idempotent push, snapshot upload and compaction, permanent purge, and size limits. (Quota enforcement belongs to `invites` from `invite-codes`.)

### Modified Capabilities
<!-- None. Consumes `accounts` sessions (from `opaque-accounts`) without changing their requirements. Snapshot and update sizes stay within the existing `server-hosting` request body limit, so that requirement is unchanged. -->

## Impact

- `notebook-server`: `/sync/v1/*` HTTP endpoints, a WebSocket endpoint at `/sync/v1/live`, `ops`, `sync_notes`, and `snapshots` tables, and a `sync_head` column on the `vaults` table from `opaque-accounts`. Vaults are unlimited until `invite-codes` adds `quota_bytes`/`used_bytes` and hooks its check into these write transactions.
- `notebook-core`: a `protocol` module with the request, response, and live-frame types (serde, wasm-compatible), so clients and server can't disagree. `sync-client-engine` builds on it.
- New config key `sync_max_update_bytes`.
- Depends on `opaque-accounts` (access-token authentication and the `vaults` table) and on the `note-documents` ciphertext format only as opaque bytes.
- Metadata visible to the server: note IDs, the number, sizes, and timing of updates and snapshots per note, and when notes are purged. This is accepted and documented.
