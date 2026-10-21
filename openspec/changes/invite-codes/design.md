# Design

## Context

See proposal.md (Why). This change lands after `opaque-accounts` and `sync-server` have been archived (TODO.md order 7, 8, then 14), so it builds on code that will exist by then:

- `crates/notebook-server` from `server-foundation`: flat `Config` with `deny_unknown_fields` and `NOTEBOOK_`-prefixed env overrides, embedded sqlx migrations, the clap `Command` enum, and `rate_limit::KeyedLimiter`, which is meant to be reused through `route_layer` for stricter per-endpoint limits.
- `opaque-accounts`: two-step OPAQUE registration (`register/start`, `register/finish`) under `/auth/*`; request/response types in `notebook-core::auth::api` and the `Registration` state machine in `notebook-core::auth::client`; the `users` (`user_id` PK), `vaults` (`vault_id` PK, unique `user_id`), `key_bundles`, and `sessions` tables; and one account-creation function, `store::create_account(tx, NewAccount)`, shared by `register/finish` and `user add` (which creates the full account: user, vault, key bundle, recovery verifier). Registration is gated by an interim `RegistrationPolicy` held in `AuthState`: `Closed` in production (both handlers return `403 {"error":"registration_closed"}` before parsing the username) and `Open` only in tests. `register/finish` runs in one `BEGIN IMMEDIATE` transaction with an extension point before `create_account`. `register/start` has a fixed per-address limiter (burst 5, one per minute).
- `sync-server`: `ops` and `snapshots` tables keyed by `vault_id`, HTTP push and snapshot upload, a WebSocket that may also carry pushes, and purge of trashed notes. Its proposal says writes are checked against "the vault storage quota"; this change is what defines that quota.

Requirements are in `specs/invites/spec.md` and `specs/accounts/spec.md` (which adds requirements to `accounts` and modifies its registration rate limit and error-code list). Workspace dependencies already include `sha2`, `getrandom`, and `ulid`.

## Goals / Non-Goals

**Goals:**
- Turn on public registration safely: gated by single-use invites, with no new surface for username enumeration or code guessing.
- One quota check used by every write path that stores vault data, with usage tracked incrementally so the check is a single-row read.

**Non-Goals:**
- In-app invites, admin web UI, or users inviting other users. The `is_admin` flag stays unused.
- Invites that grant more than one account, or carry per-invite quotas.
- Quotas on anything other than encrypted note data (users, sessions, key bundles are small and bounded per account).
- Client UI for entering codes; that belongs to `desktop-app-shell` and `browser-web-client`. This change only adds the field to the shared request type and core registration call.
- Pruning old invite rows. They are tiny and serve as an audit trail.

## Decisions

### Code format and hashing

A code is 20 characters of Crockford base32 (100 bits from `getrandom`), printed as four dash-separated groups of five, e.g. `7KQ2M-XW9PD-0RT4H-C6NBJ`. Crockford's alphabet omits `I`, `L`, `O`, `U`; on redemption the input is trimmed, dashes removed, upper-cased, and `I`/`L` mapped to `1` and `O` to `0`, as the alphabet defines. Encoding is a dozen lines in the server; no base32 crate.

Stored as `SHA-256(normalized code)`, looked up by that hash with a unique index. A fast hash is enough because the input has 100 bits of entropy; a slow KDF (Argon2) would only add per-request CPU that an attacker can trigger. Alternative considered: storing a code ID prefix plus a hashed secret (like API tokens) — unnecessary, since an indexed lookup on the full hash already avoids comparing secrets byte by byte.

### Schema

```sql
CREATE TABLE invites (
  id            BLOB PRIMARY KEY,          -- 16-byte ULID
  code_hash     BLOB NOT NULL UNIQUE,      -- SHA-256 of normalized code
  note          TEXT,
  created_at    INTEGER NOT NULL,          -- unix seconds
  expires_at    INTEGER NOT NULL,
  revoked_at    INTEGER,
  used_at       INTEGER,
  used_by       BLOB REFERENCES users(user_id)  -- user created with it
);

ALTER TABLE vaults ADD COLUMN quota_bytes INTEGER NOT NULL DEFAULT 104857600;
ALTER TABLE vaults ADD COLUMN used_bytes  INTEGER NOT NULL DEFAULT 0;
```

`invites` is not vault data, so the `vault_id` convention does not apply. Status is derived (`revoked_at`, then `used_at`, then `expires_at` vs now), never stored, so expiry needs no background job.

"Who created it" is the operator CLI, the only creator; `note` lets the operator record who the code is for. A `created_by` column would always hold the same value, so it is left out until in-app invites exist.

The migration backfills `used_bytes` from `SUM(length(payload))` over `ops` and `snapshots` per vault, and sets `quota_bytes` for existing rows, including vaults created by `user add`. SQLite `ALTER TABLE ... ADD COLUMN` needs a constant default, but the configured default is a runtime value: the migration uses the built-in default (100 MiB), and new vaults get the configured value explicitly at insert time. `store::create_account` gains a `quota_bytes` argument that it writes into the `vaults` row (rather than a field on `NewAccount`, which is the client-side result type from core and should not know about server quotas); both callers (`register/finish` and `user add`) fill it from `default_vault_quota_bytes`, so CLI-created and self-registered vaults cannot get different defaults. If `sync-server` has already added either column, the migration skips that column (the task list checks this when implementing).

The CLI shows the ULID in short form (its last 8 characters, matched as a unique suffix) so `invite revoke` is easy to type; an ambiguous suffix is an error.

### Atomic consumption

`register/finish` already opens one `BEGIN IMMEDIATE` transaction and passes it through an extension point before `store::create_account`. This change fills that hook with the invite consumption, so it is the first statement of the transaction:

```sql
UPDATE invites SET used_at = ?now, used_by = ?user_id
 WHERE code_hash = ?h AND used_at IS NULL AND revoked_at IS NULL AND expires_at > ?now
```

If `rows_affected != 1`, the transaction rolls back with the invalid-invite error. Because the transaction already holds SQLite's write lock from `BEGIN IMMEDIATE`, concurrent finishes serialize; the second one sees `used_at` set and affects zero rows. `user_id` is generated before the hook runs, so the hook can record it in `used_by` and `create_account` inserts the same ID. A `username_taken` (or malformed-bundle) failure later in the same transaction rolls the invite update back with it, which is what keeps the invite redeemable. `user add` calls `create_account` directly, never the hook, so it neither needs nor consumes an invite.

Alternative considered: reserving the invite at registration start. Rejected because OPAQUE start is stateless and an abandoned start would strand the code.

### Replacing the interim registration policy

`RegistrationPolicy` (`Closed` / test-only `Open`) and the `registration_closed` error are removed. In their place, both registration handlers run the invite check at the same point the policy check ran, before parsing the username: `register/start` with a read-only query (same predicate as above), `register/finish` with the consuming update in the transaction hook. So registration is open exactly when a valid invite is presented, and there is still no configuration switch that opens public sign-up. `opaque-accounts` tests that used `RegistrationPolicy::Open` create an invite through the store and pass its code instead.

Both registration request types in `notebook-core::auth::api` gain a required `invite_code`. Checking before the username is parsed means an attacker without a code learns nothing about usernames (`invalid_username` or `username_taken`) and cannot make the server do OPAQUE work. Every failure (malformed, unknown, expired, revoked, used, missing) maps to one response: `403 {"error":"invalid_invite"}`, added to the `accounts` uniform error-code list. The comparison is an indexed lookup on the full hash, so timing does not depend on how close a guess is.

### Rate limiting

No new limiter. The existing `register/start` limiter from `opaque-accounts` is reused and changed in two ways:

- Its burst and refill come from new config keys `registration_rate_limit_burst` (default 5) and `registration_rate_limit_per_minute` (default 1) instead of constants, so defaults are unchanged.
- The same limiter instance is also attached to `register/finish`. `opaque-accounts` left finish to the global limiter because finish needs a handshake ID from a limited start, but registration has no pending handshake: one start's response can be replayed into many finishes, each now carrying an invite code to check. Sharing one bucket between start and finish closes that, and a legitimate registration uses only two tokens.

It runs as a `route_layer` before the handler, so requests over the limit never touch the database, and it stays separate from the login and recovery limiters. Guessing is not the real defence (100 bits of entropy is); the limit bounds database and OPAQUE work per address.

### Quota accounting and enforcement

A small `quota` module owns both:

- `reserve(tx, vault_id, delta_bytes) -> Result<(), QuotaExceeded>` runs `UPDATE vaults SET used_bytes = used_bytes + ?d WHERE vault_id = ? AND (?d <= 0 OR used_bytes + ?d <= quota_bytes)` inside the caller's write transaction and checks `rows_affected`. One statement, so there is no read-then-write race between concurrent pushes.
- Every sync write path computes its net delta and calls it in the same transaction that inserts or deletes rows: push (`+payload`), snapshot upload (`+snapshot − bytes of compacted ops and replaced snapshot`), purge (`−all of the note's bytes`). Non-positive deltas always pass, which is what lets compaction and purge proceed at or over quota.
- Over HTTP, `QuotaExceeded` maps to `507 Insufficient Storage` with `{"error":"quota_exceeded"}`. `413` was rejected because it already means "this request body is too large", which clients treat as non-retryable for that payload, whereas a quota error clears when the user deletes notes or the operator raises the quota. Over WebSocket it maps to the sync protocol's per-push error frame with code `quota_exceeded`; the connection stays open.

Usage counts payload bytes only (ciphertext length), not row overhead or indexes, so it is predictable and the same number clients could compute.

Alternative considered: computing usage on demand with `SUM()` per write. Simple, but cost grows with vault size and it races without a lock. The counter plus a backfill is cheap and exact as long as every write goes through `reserve`; a test asserts the counter equals the recomputed sum after a mixed workload.

### Configuration

New flat keys, following `server-foundation`'s table:

| key | type | default |
|-----|------|---------|
| `default_vault_quota_bytes` | integer (bytes) | `104857600` (100 MiB) |
| `registration_rate_limit_burst` | integer | `5` |
| `registration_rate_limit_per_minute` | integer | `1` |

### CLI

```
notebook-server invite create [--expires <dur>] [--note <text>]
notebook-server invite list [--all]
notebook-server invite revoke <id>
notebook-server user quota <username> [<size>]
```

`invite` is a new subcommand group; `quota` joins `user add` under the `user` group from `opaque-accounts`. Durations accept an integer with `m`, `h`, or `d` (bounded to 1 minute .. 365 days); sizes accept a plain byte count or `KiB`/`MiB`/`GiB`. Both are parsed by small hand-written functions with clap `value_parser`, not `humantime`/`bytesize`, given three units each. `invite create` prints only the code on stdout (so `$(notebook-server invite create)` works) and the expiry on stderr. Times are shown in UTC RFC 3339.

These commands open the database directly, like `migrate`, and run pending migrations first, so they work whether or not `serve` is running (SQLite WAL allows it).

### Client side

Both registration request types in `notebook-core::auth::api` gain a required `invite_code: String`, and `Registration::start(username, passphrase, invite_code)` in `notebook-core::auth::client` keeps the code in its state and puts it in both `RegisterStartRequest` and `RegisterFinishRequest`. The client sends the code as typed; normalization is the server's job so the rules live in one place. `invalid_invite` is added to the core auth error codes, and `quota_exceeded` to the sync protocol's error codes, so the app shells and `sync-client` can match on them. `user add` drives `Registration` in-process and hands its messages straight to `ServerRegistration` and `create_account`, never to the HTTP handlers, so it passes an empty code that nothing checks.

## Risks / Trade-offs

- [A sync write path that forgets `reserve` lets usage drift] → All writes to `ops`/`snapshots` go through functions in one module that take the transaction; an integration test compares `used_bytes` with a recomputed sum after push, compaction, and purge.
- [`sync-server` may already ship its own quota stub or columns] → The first task checks what landed and adapts (reuse columns, replace the stub with `reserve`), rather than adding parallel state.
- [100 bits of entropy plus 1 attempt/minute/IP is overkill for guessing, but IPv6 /48 aggregation still lets a large botnet try many codes] → Even at 10⁹ guesses/day across all addresses, expected time to hit one of a handful of live codes is astronomically long; expiry bounds the window further.
- [Distributing codes out of band (chat, email) can leak them] → Single use plus short expiry; the operator can revoke, and `invite list --all` shows who redeemed each code.
- [`507` is uncommon and some HTTP stacks treat 5xx as retryable] → The body carries `quota_exceeded`; `sync-client` must treat it as non-retryable until usage changes (noted as a cross-change dependency).
- [Lowering a quota below usage can leave a client with an undeliverable outbox] → Expected; the user can delete or purge notes, which always succeeds, and the client surfaces the error.

## Migration Plan

1. Deploy the new binary; `serve` (or `migrate`) creates `invites`, adds and backfills the quota columns. Existing users keep working and get the default quota.
2. Public registration is now open but requires a code; operators run `invite create` to onboard people.
3. Rollback: the older binary refuses the newer schema (by `server-foundation` design). Restore the Litestream backup taken before the upgrade.

## Open Questions

- Whether clients should be able to read their vault's usage and quota (e.g. in account settings). Not needed for enforcement; can be added later as a read-only field on an account info response without changing anything here.
