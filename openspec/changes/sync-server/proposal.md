# Proposal

## Why

Editing the same note from the native app and the browser requires the server to relay encrypted CRDT updates between a user's clients. Because the server can't read or merge content, it acts as a durable, ordered, encrypted mailbox. This change defines that protocol and the server's side of it.

## What Changes

- An append-only, per-vault **operation log**: clients push encrypted CRDT updates tagged with note ID. The server assigns a monotonically increasing sequence number and stores them.
- **Bootstrap** endpoint: returns, for every note in the vault, the latest encrypted snapshot plus any encrypted updates after it. Used by the thin browser client on login and by a fresh native install.
- **Incremental pull**: return all updates after a client's cursor.
- **Live channel** (WebSocket): authenticated clients receive newly appended updates for their vault in real time. Pushes may also go over the socket.
- **Snapshots and compaction**: a client can upload an encrypted snapshot declaring "covers note N through sequence S". The server stores it and may delete that note's older updates. The server never creates snapshots itself.
- **Idempotency**: each pushed update carries a client-generated ID so retries don't duplicate entries.
- Note deletion propagates as an encrypted update. Permanent purge of a trashed note removes its log and snapshots server-side.
- Enforce authentication, per-vault isolation, payload size limits, and the vault storage quota on every write.

## Capabilities

### New Capabilities
- `sync-protocol`: The client-server sync contract: encrypted op log, sequence ordering, bootstrap, incremental pull, live updates, idempotent push, snapshot upload and compaction, and quota/size enforcement.

### Modified Capabilities
<!-- None. Consumes `accounts` sessions and `invites` quotas without changing their requirements. -->

## Impact

- `notebook-server`: `/sync/*` HTTP endpoints, a WebSocket endpoint, and `ops` and `snapshots` tables.
- Shared protocol types (request/response shapes, framing) live in `notebook-core` so clients and server can't disagree.
- Metadata visible to the server: note IDs, update counts and sizes, and timing. This is accepted and documented.
