# Design

## Context

See proposal.md (Why). `notebook-server` (from `server-foundation`) is an axum 0.8 app with an `AppState { pool, config, limiter }`, a `harden()` middleware stack (security headers, tracing without bodies or query strings, client IP, global per-IP rate limit, request timeout, a global body limit defaulting to 1 MiB), sqlx 0.9 on one SQLite file in WAL mode, and embedded migrations under `migrations/` that so far contain only the `vault_id` convention README. `opaque-accounts` lands first and adds the `vaults` table, access-token sessions, and an authenticated-request extractor. `invite-codes` lands later and owns vault quotas: it adds `vaults.quota_bytes`/`used_bytes` (backfilled from `SUM(length(payload))` over `ops` and `snapshots`) and a `reserve(tx, vault_id, delta)` call that every sync write path invokes inside its transaction.

On the client side, `crdt-note-documents` fixes the payloads: each update and snapshot is a `vault-crypto` envelope bound to (vault, note, kind), and a note key is wrapped by the vault key (envelope kind 16). The server sees only bytes. The vault key bundle (vault key wrapped by the auth secret and recovery key) is `opaque-accounts`' and is served by its `GET /auth/key-bundle`; this change stores only the per-note wrapped note keys, which no other change carries. Closing live sockets whose session is revoked (logout, device revocation, passphrase change) is this change's job, using `opaque-accounts`' `sessions::is_active(pool, session_id)`. Requirements are in `specs/sync-protocol/spec.md`.

## Goals / Non-Goals

**Goals:**
- A protocol where a client that follows pull cursors and the live channel can never miss a change, even across compaction, pagination, and reconnects.
- Every write is one SQLite transaction with a computed net payload-byte delta, so sequence assignment and compaction never disagree and `invite-codes` can drop its quota check into one place per write path.
- Protocol types live once, in `notebook-core`, and compile for wasm32.

**Non-Goals:**
- Client behavior: outbox, retry and backoff, when to snapshot, how to react to close codes (`sync-client-engine`).
- Storage accounting, quotas, and the `quota_exceeded` error (`invite-codes`).
- Hiding metadata (sizes, timing, note count) from the server.
- Detecting a rolled-back server beyond the cursor-ahead check (see Risks).
- Multi-process servers. One `notebook-server` process owns the database.

## Decisions

### Endpoints

All under `/sync/v1`, authenticated with the `accounts` access-token extractor. The vault comes from the token, never the request.

| method + path | body | success |
|---|---|---|
| `POST /sync/v1/push` | JSON `PushRequest` | `200` JSON `PushResponse` |
| `GET /sync/v1/changes?after=<seq>` | none | `200` JSON `ChangesResponse` |
| `PUT /sync/v1/notes/{note_id}/snapshot?covers=<seq>` | raw ciphertext, `application/octet-stream` | `204` |
| `DELETE /sync/v1/notes/{note_id}` | none | `204` |
| `GET /sync/v1/live` | WebSocket upgrade (pushes may also go over it) | `101` |

The `v1` path segment versions the protocol. A breaking change adds `/sync/v2` alongside it.

Bootstrap is not a separate endpoint: it is `changes?after=0`. One code path means bootstrap and incremental pull can't drift apart, and a client that falls behind a compaction recovers through the same logic as a fresh install.

### Wire encoding: JSON with base64 payloads

Requests and responses are JSON. Payload bytes (updates, snapshots, wrapped keys) are standard base64 with padding. IDs are the 26-character ULID text forms that `NoteId` and `VaultId` already parse. Sequence numbers are JSON integers (u64, always below 2^53).

Snapshot upload is the exception: its body is the raw ciphertext, so the largest payload doesn't pay the base64 overhead against the body limit. The covered sequence goes in the query string, which the request log already omits.

**Alternative considered: CBOR or postcard everywhere.** That saves the ~33% base64 overhead on pulls, and since the sync engine runs in Rust/WASM on every client, JS never parses the wire format. Rejected for v1: JSON is what every other endpoint (errors, auth) speaks, it's debuggable with `curl`, `serde_json` is already in the workspace, and at personal-vault scale the overhead is tolerable. A binary encoding can arrive as `/sync/v2` or content negotiation.

### Shared types in `notebook_core::protocol`

`notebook-core` gains `serde` (derive) and `base64` dependencies, both wasm-clean. The module holds `PushRequest { updates: Vec<PushUpdate> }`, `PushUpdate { id: OpId, note: NoteId, data: Bytes, note_key: Option<Bytes> }`, `PushResponse { seqs: Vec<u64>, head: u64 }`, `ChangesResponse { notes: Vec<NoteEntry>, entries: Vec<Entry>, cursor: u64, more: bool }`, `NoteEntry { note, key: Option<Bytes>, snapshot: Option<Snapshot { covers, data }> }`, `Entry` = `Update { seq, id: OpId, note, data, note_key: Option<Bytes> }` | `Purge { seq, note }` (JSON tag `kind`: `"update"` / `"purge"`), the live frames (including `push` and `push_result`), the error body `{ "error": "<code>" }` with an `ErrorCode` enum, and the close-code constants. `Bytes` is a newtype that serializes as base64. `OpId` is a ULID newtype generated by the client.

The module has no networking; `sync-client-engine` adds transports on top. Round-trip serde tests in core pin the JSON field names so a rename fails CI.

### Schema

One migration, `0002_sync.sql` (numbered after `opaque-accounts`' migration), following the `vault_id` convention:

```sql
ALTER TABLE vaults ADD COLUMN sync_head INTEGER NOT NULL DEFAULT 0;

CREATE TABLE sync_notes (
  vault_id    BLOB NOT NULL REFERENCES vaults(id) ON DELETE CASCADE,
  note_id     BLOB NOT NULL,
  wrapped_key BLOB,              -- NULL once purged
  created_seq INTEGER NOT NULL,  -- seq of the registering op
  purged_seq  INTEGER,           -- NULL unless purged (tombstone)
  PRIMARY KEY (vault_id, note_id)
) WITHOUT ROWID;

CREATE TABLE ops (
  vault_id BLOB NOT NULL REFERENCES vaults(id) ON DELETE CASCADE,
  seq      INTEGER NOT NULL,
  op_id    BLOB NOT NULL,
  note_id  BLOB NOT NULL,
  payload  BLOB NOT NULL,
  PRIMARY KEY (vault_id, seq)
) WITHOUT ROWID;
CREATE UNIQUE INDEX ops_op_id ON ops (vault_id, op_id);
CREATE INDEX ops_note ON ops (vault_id, note_id, seq);

CREATE TABLE snapshots (
  vault_id BLOB NOT NULL,
  note_id  BLOB NOT NULL,
  covers   INTEGER NOT NULL,
  payload  BLOB NOT NULL,
  PRIMARY KEY (vault_id, note_id),
  FOREIGN KEY (vault_id, note_id) REFERENCES sync_notes(vault_id, note_id) ON DELETE CASCADE
) WITHOUT ROWID;
CREATE INDEX snapshots_covers ON snapshots (vault_id, covers);
CREATE INDEX sync_notes_created ON sync_notes (vault_id, created_seq);
CREATE INDEX sync_notes_purged ON sync_notes (vault_id, purged_seq) WHERE purged_seq IS NOT NULL;
```

`sync_head` is the last sequence assigned, stored rather than computed as `MAX(seq)` because compaction and purge delete ops. The blob column is named `payload` in both `ops` and `snapshots` because `invite-codes`' migration backfills usage from `SUM(length(payload))` over those two tables. No quota or usage columns are added here; `invite-codes` adds them.

### Serializing writes

Every write (push, snapshot, purge) runs in a transaction opened with `BEGIN IMMEDIATE` (sqlx `begin_with`), so it takes SQLite's write lock up front instead of failing with `SQLITE_BUSY` on upgrade. Inside: read `sync_head`; validate; insert/delete; update `sync_head`; commit.

All three live in one `sync::store` module whose write functions take the open transaction and compute the write's **net payload-byte delta** (push: `+Σ new payloads`; snapshot: `+snapshot − replaced snapshot − compacted ops`; purge: `−all of the note's payloads`). Each function has a single, commented point just before commit where that delta is known, which is where `invite-codes` calls `reserve`. HTTP and WebSocket pushes share the same function, so the hook covers both. This change ships no quota stub.

Live delivery must be in sequence order. Two pushes that commit in order A, B could otherwise broadcast B then A. So each vault also has an in-process `tokio::sync::Mutex` (in a `DashMap<VaultId, Arc<VaultHub>>` on `AppState`), held across begin, commit, and broadcast. SQLite still guarantees correctness if a second process ever wrote; the mutex only orders broadcasts and avoids lock contention.

### Push

For each update in request order: validate size (`sync_max_update_bytes`, default 256 KiB, a new config key) and IDs; look up `op_id`. If present with the same note and bytes, reuse its seq; if present with different content, fail the batch with `op_id_conflict`. If the note is purged, fail with `note_purged`. If the note is unregistered, require `note_key` and insert into `sync_notes` with `created_seq` = the op's new seq; if registered and `note_key` is present, compare. New ops get `sync_head + 1`, `+ 2`, … Any failure rolls back the whole batch, and nothing is broadcast.

Duplicate `op_id`s within one batch are rejected as `400`, as are batches of more than 128 updates (`protocol::MAX_PUSH_UPDATES`) and wrapped keys longer than 256 bytes (`MAX_NOTE_KEY_BYTES`; a real one is 32 bytes plus envelope overhead). The batch cap bounds how long one push holds SQLite's write lock and keeps a batch's entries inside the 256-entry live buffer, so an outbox flush doesn't lag every socket of the vault.

The write and its broadcast run in a spawned task that the handler awaits. If the request is cancelled (client disconnect, request timeout) after the commit was sent, the commit still lands, and the broadcast must too, or live connections would silently skip those sequence numbers. Purge works the same way.

**Idempotency after compaction.** The `op_id` index only covers ops still in the log. If a client's retry arrives after its op was compacted away, the op is stored again under a new seq. That's harmless: the CRDT applies duplicate updates as no-ops (see `note-documents` Convergence), and the next snapshot removes it. Keeping op IDs forever (or for a time window) in a separate table was considered and rejected as unbounded growth or a tuning knob to guard a case the CRDT already absorbs.

### Pull and pagination

`changes?after=C` runs in one read transaction (WAL gives a consistent snapshot). It merges three streams, each keyed by a vault sequence number above C:

- ops with `seq > C`,
- snapshots with `covers > C`,
- note registrations with `created_seq > C` (the wrapped key) and tombstones with `purged_seq > C`.

Items are grouped by key and whole groups are emitted in key order while they fit the page's budget (2 MiB, a constant, counting each item's payload plus a fixed 64-byte overhead), so no key is split across pages; the first group is emitted even if it alone is over budget. Sizing reads lengths only, a 1024-sequence-number window at a time with indexed `min()` lookups to skip gaps, so a page's cost doesn't grow with the vault; then the page's payloads are fetched by key range, all in the same read transaction. `cursor` is the last key included (or `sync_head` when the page drains everything, so a client with no changes still advances past purges and compactions), and `more` says whether items remain. A single item larger than the budget is returned alone.

Why this has no gaps: a snapshot covering S for note N exists only after N's ops `≤ S` were deleted in the same transaction. A client at cursor C < S gets that snapshot (its key S > C) plus N's ops above S. A client at C ≥ S already received those ops when they were live. A wrapped key's `created_seq` is ≤ every seq of that note's ops and ≤ its snapshot's `covers`, so it arrives in the same page or an earlier one than any data for the note. Purged notes have no ops, snapshot, or key left, so only their tombstone (a `purge` entry at `purged_seq`) is returned, and only to clients whose cursor predates the purge. Bootstrap (C = 0) therefore skips nothing and returns nothing for purged notes beyond their `purge` entries.

Between pages, a compaction may land. That is still safe: the new snapshot's `covers` is above the deleted ops, and either the client already received those ops (`covers ≤` its cursor) or the snapshot's key is above the cursor and arrives on a later page.

`after > sync_head` returns `409 cursor_ahead`.

### Snapshot upload

`PUT …/snapshot?covers=S` with the raw body. Checks: note registered and not purged (`404` / `410`), `S ≤ sync_head` and `S ≥` the note's `created_seq` (`400`; a snapshot keyed below the registration could reach a client before the note's key, and `S = 0` would never be pulled), `S >` the existing snapshot's `covers` (`409 snapshot_stale`). Then replace the snapshot row and `DELETE FROM ops WHERE vault_id = ? AND note_id = ? AND seq <= S`, computing the net delta from the replaced and deleted payload lengths (the delta is usually negative, which `invite-codes` always allows, so a vault at quota can compact its way back under).

A snapshot's size is bounded by the global request body limit (`max_body_bytes`, 1 MiB by default), so `server-hosting` stays unchanged. No snapshot broadcast is sent; live clients already have the ops it covers.

Two devices snapshotting the same note concurrently: the first wins and the second gets `409 snapshot_stale`, which `sync-client-engine` handles as a rejected snapshot (keep local updates, retry after more arrive). Accepting the older one instead would mean either keeping two snapshots or re-materializing ops it compacted.

The server can't check that the snapshot really contains every op it claims to cover. A buggy or malicious client of the vault can only damage its own vault, which it can do anyway by pushing garbage updates.

### Purge

`DELETE /sync/v1/notes/{id}`: delete the note's ops and snapshot, set `wrapped_key = NULL` and `purged_seq = sync_head + 1`, bump `sync_head`, compute the (negative) delta, broadcast its `purge` entry. Already purged → `204`, no change. Unknown note → `404`. Tombstones are kept forever (about 40 bytes each) so a device that was offline for months still learns of the purge and gets `410` on resurrecting pushes.

Trashing (soft delete) is just a CRDT update and needs nothing here.

### Live channel

`GET /sync/v1/live` upgrades with axum's `WebSocketUpgrade`. Before upgrading, if an `Origin` header is present it must equal the origin of `public_url`, else `403`, to stop cross-site WebSocket hijacking. Native clients send no `Origin`.

Frames are JSON text messages:

- client → server: `{"type":"auth","token":"…"}` first, within 10 s; afterwards `{"type":"push","request_id":"…","updates":[…PushUpdate]}`. Incoming messages are capped at `max_body_bytes`, the same bound an HTTP push has. Unknown message types are ignored.
- server → client: `{"type":"ready","head":H}`, then `{"type":"entry", …Entry}` (the same update and purge entries a pull returns), and for each push `{"type":"push_result","request_id":"…","seqs":[…],"head":H}` or `{"type":"push_result","request_id":"…","error":"<code>"}`.

A push frame runs the same store function as `POST /sync/v1/push`, so validation, atomicity, idempotency, and the quota hook are identical. A rejected push (`update_too_large`, `op_id_conflict`, `note_purged`, `quota_exceeded` once `invite-codes` lands, …) returns an error `push_result` and the socket stays open; only protocol violations (push before auth, unparseable frame) close it. The pushing connection also receives its own updates as `entry` frames, like every other subscriber; the client deduplicates by op ID or seq. Socket pushes are counted against the per-IP limiter (`KeyedLimiter::check` on the connection's client IP per push frame), since they bypass the HTTP middleware.

On `auth`, the server validates the token with the same lookup `opaque-accounts`' `AuthSession` extractor performs (factored out as a function taking the token string, since the extractor reads `Authorization: Bearer`, which browsers cannot send on a WebSocket; HTTP sync routes use the extractor itself), subscribes the connection to the vault's `tokio::sync::broadcast` channel (capacity 256), reads `sync_head`, then sends `ready`. Because the subscription happens before the head is read, any change committed after the read is on the channel, and the client's HTTP pull from its cursor covers everything up to H. Changes may arrive on both paths; the client drops entries whose seq is at or below its cursor. The live channel deliberately takes no cursor: "subscribe from a cursor" (as `sync-client-engine` puts it) is `ready` followed by an HTTP pull from that cursor, so catch-up pagination exists in exactly one place.

Tokens go in the first message, not the query string, because browsers can't set an `Authorization` header on a WebSocket and URLs end up in proxy logs. The token is never logged.

Close codes (application range): `4001` authentication failed or token expired (refresh and reconnect), `4002` resynchronize (pull, then reconnect), `4003` too many connections, `4004` server shutting down. Access tokens live 15 minutes, so a client keeps a socket by sending another `auth` frame with its refreshed token; the server accepts it only if it resolves to the same vault, and moves the deadline to the new expiry. On every ping tick the server also calls `sessions::is_active(session_id)` and closes with `4001` if the session was revoked. The server closes at the current token's expiry time, closes with `4002` when `broadcast::RecvError::Lagged` occurs, pings every 30 s and closes after two unanswered pings (`1001`), and caps live connections at 16 per vault. Each socket write times out after one ping interval and drops the connection, so a client that stops reading can't block the connection's loop (expiry, revocation, and shutdown checks) or pin the vault's broadcast buffer; the loop polls those closing conditions ahead of new entries. An unparseable frame after auth closes with `1008`.

**Alternative considered: HTTP-only pushes, socket server-to-client only.** One fewer code path, and `harden()`'s body and rate limits apply for free. Rejected because `invite-codes` and `sync-client-engine` expect pushes over the socket, and sharing one store function keeps the duplication to frame parsing plus an explicit rate-limit check.

On graceful shutdown, live connections are sent `4004` so they don't hold the process for the full grace period.

### Errors

`SyncError` maps to status and code: `400 bad_request`, `401 invalid_token` (the code `opaque-accounts` uses), `404 unknown_note`, `409 op_id_conflict | note_key_conflict | snapshot_stale | cursor_ahead`, `410 note_purged`, `413 update_too_large` (and the existing body-limit `413`), `422 missing_note_key`. `invite-codes` adds `507 quota_exceeded`. Bodies are `{"error":"<code>"}`; WebSocket push errors carry the same code in `push_result.error`. Database errors become `500` with a generic body and are logged without payloads. Purged notes get their own code (`410 note_purged`) rather than `unknown_note`, so a client can tell "drop this note, it was purged" from "this note was never registered" (a client bug); `sync-client-engine` treats both as "unknown/purged note". If `opaque-accounts` settles on a different error-body shape, sync adopts it; the codes stay.

## Risks / Trade-offs

- [Base64 inflates pulls by a third] → Acceptable at personal scale; snapshots compact history, and a binary encoding can be added as `/sync/v2`.
- [A note whose CRDT state exceeds the body limit can't be snapshotted, so its log grows] → Rare for markdown notes; the operator can raise `max_body_bytes`. The log stays correct, only less compact.
- [A server restored from backup may reuse sequence numbers a client already saw if new pushes land before that client reconnects] → The cursor-ahead check catches the common case (client cursor above the restored head). Full detection needs a per-vault epoch ID, deferred until restores are more than an operator emergency; `docs/deployment.md` tells operators to have users re-bootstrap after a restore.
- [Idempotency doesn't survive compaction] → Retried duplicates are CRDT no-ops and get compacted again.
- [In-process mutex assumes a single server process] → Documented; the deployment model is one process per database.
- [One account can open many sockets and hold memory, or push over a socket to dodge HTTP limits] → 16 connections per vault, incoming frames capped at `max_body_bytes`, 10 s auth deadline, per-IP rate limit on upgrades and on each socket push.
- [A write path skips `invite-codes`' quota hook] → All writes go through `sync::store` functions with one marked hook point each; `invite-codes` has an integration test comparing `used_bytes` with recomputed sums.
- [The live channel reveals timing of edits in real time] → No worse than HTTP pushes; already accepted in the threat model.

## Migration Plan

New tables and new nullable/defaulted columns only; no existing data to migrate. The migration must sort after `opaque-accounts`' migration, since it alters `vaults`. Rollback is restoring the pre-upgrade database backup, as `server-foundation` documents.
