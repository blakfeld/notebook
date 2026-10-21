# Design

## Context

See proposal.md (Why). Requirements are in `specs/accounts/spec.md` and the `vault-crypto` delta.

Current state:
- `notebook-core::crypto` (from `vault-encryption`) already has `VaultId`, `VaultKey`, `AuthSecret::from_bytes` (≥ 32 bytes; OPAQUE's export key is 64), `RecoveryKey` (16 random bytes plus a 4-byte checksum), `KeyBundle` with `create`, `unlock_with_auth`, `unlock_with_recovery`, `change_auth(old, new)`, `recover_auth(recovery, new)`, `to_bytes`/`from_bytes`, and Argon2id `KdfVersion::V1` (64 MiB, t = 3, p = 1). Core has no serde and no I/O.
- `notebook-server` (from `server-foundation`) has `AppState { pool, config, limiter }`, `app::router()` with a `harden()` middleware stack, `KeyedLimiter` (IP-keyed token buckets with IPv6 prefix aggregation) attachable per route with `route_layer`, flat `Config` with `deny_unknown_fields`, sqlx/SQLite with embedded migrations (only a README so far, stating the `vault_id` convention), a clap CLI with `serve` and `migrate`, and `docs/deployment.md` covering Litestream backups.
- Downstream: `invite-codes` adds an invite requirement to registration and `user quota` CLI commands (its `accounts` delta assumes registration has start and finish steps and that `user add` exists); `sync-server` authenticates every `/sync` request and WebSocket with a session; `sync-client-engine` refreshes sessions; the app shells drive the flows.

## Goals / Non-Goals

**Goals:**
- One client implementation of every auth flow, in `notebook-core`, used by the desktop app, mobile app, browser (wasm), the server's `user add` CLI, and server integration tests.
- Request/response types defined once in core so client and server cannot disagree.
- A stolen database alone does not allow offline passphrase guessing.
- A small extractor that later endpoints use to get `(user_id, vault_id, session_id)`.

**Non-Goals:**
- HTTP client code. The flows are sans-I/O state machines; `sync-client-engine`'s transport and the app shells send the requests.
- UI for any flow (app-shell, web-client, mobile changes).
- Invite codes and quotas (`invite-codes`).
- Rotating the OPAQUE server key or the recovery key, deleting accounts, renaming usernames, and an admin HTTP API. The schema leaves room for key rotation (see below).
- Closing open WebSockets when a session is revoked. The session-check API here makes it possible; `sync-server` owns its sockets.
- Multi-instance deployment. Pending handshakes and rate limiters are in memory, matching `server-foundation`.

## Decisions

### Module layout

```
crates/notebook-core/src/auth/
  mod.rs        AuthError, re-exports
  suite.rs      the opaque-ke CipherSuite (shared by client and server)
  username.rs   Username: validation and lowercase normalization
  api.rs        serde request/response types, base64url byte fields
  client.rs     Registration, Login, PassphraseChange, Recovery state machines
crates/notebook-core/src/crypto/recovery.rs   + RecoveryKey::verifier()
crates/notebook-core/src/crypto/bundle.rs     + KeyBundle::same_recovery_entry()

crates/notebook-server/src/auth/
  mod.rs        routes(), AuthState (server setup, pending map, limiters)
  keyfile.rs    load/create the OPAQUE key file, fingerprint check
  handlers.rs   /auth/* handlers
  sessions.rs   token issue/refresh/revoke, authenticate_token, AuthSession extractor, cleanup task
  store.rs      SQL for users, vaults, key_bundles, sessions, server_meta
  pending.rs    TTL map for in-flight handshakes
  admission.rs  RegistrationPolicy
crates/notebook-server/src/cli.rs              + User { Add { .. } }
crates/notebook-server/migrations/0001_accounts.sql
```

The server-side OPAQUE calls (`ServerRegistration`, `ServerLogin`) live in the server crate; core only exposes the cipher suite type so both sides agree on it. `opaque-ke` is a dependency of both crates.

### OPAQUE cipher suite

`opaque-ke` (latest stable at implementation, expected 4.x; pin the exact version) with:
- OPRF and key-exchange group: ristretto255; key exchange: TripleDH with SHA-512.
- Key stretching (KSF): Argon2id with the `vault-crypto` V1 parameters (64 MiB, t = 3, p = 1), via a small `Ksf` wrapper around the workspace `argon2` crate, so stretching settings are the vetted ones and the `opaque-ke` `argon2` feature (which may pull a second `argon2` version) is not needed.

Alternatives: P-256 (larger, slower in wasm, no benefit here); opaque-ke's default Argon2 parameters (19 MiB, t = 2; weaker than what `vault-crypto` already chose). Stretching runs on the client, once per login, so its cost is paid by the user's device, not the server. If both the database and the key file are stolen, Argon2 is what slows per-user guessing.

Identifiers: the OPAQUE credential identifier is the normalized username bytes. Client and server identities use the protocol defaults (the public keys), so changing the server's domain does not invalidate records.

The export key (64 bytes) becomes `AuthSecret::from_bytes(export_key)`. The OPAQUE session key is not used; the server issues its own tokens.

### OPAQUE server key file

`ServerSetup` (OPRF seed, server key pair, fake-record key pair) is serialized to a key file at a new config key `opaque_key_path` (default `opaque-server.key`, resolved like `database_path`). It is deliberately not in the database: Litestream replicates the database to object storage, and a copy of the database without the OPRF seed is useless for offline guessing. With both, an attacker can guess offline (inherent to OPAQUE), slowed by Argon2.

Startup (`serve` and `user add`; `migrate` does not touch it):
1. If the file exists, load it (bad format → exit non-zero naming `opaque_key_path`).
2. If absent and `users` is empty, generate, write with mode `0600` via create-new (never overwrite), and continue.
3. If absent and users exist → refuse (spec). Auto-regenerating would silently lock out every user.
4. Compare `SHA-256(server public key)` with `server_meta.opaque_key_fingerprint`; insert it if absent, refuse on mismatch.

`users.server_key_id` (always 1 now) records which setup a user's record belongs to. Rotation, later, is: load several key files by ID, register new records under the newest, and move a user to it on their next passphrase change or recovery. Not built now because the only reason to rotate is key-file compromise, and recovery already works under a new key (it does not need the old record).

`docs/deployment.md` gains: the key file's location and permissions, that it must be backed up separately from (and not alongside) the Litestream replica, and what losing it means (every user must recover with their recovery key; data is not lost). This change ships no command for replacing a lost key; the documented answer is to restore the key file from its backup. A guarded "regenerate and force everyone through recovery" command is deferred to the key-rotation work, since an environment flag or `--force` that is easy to leave set would defeat the refusal.

### Usernames

ASCII-only (`[a-z0-9][a-z0-9._-]{2,31}` after lowercasing). ASCII avoids Unicode normalization and confusables entirely; usernames are only ever seen by their owner and the operator. `Username::parse` lives in core so the client normalizes before deriving the OPAQUE credential identifier; the server parses again and never trusts the client's normalization.

### Wire protocol

JSON over HTTP, `Content-Type: application/json`. Byte fields are base64url without padding. All routes are under `/auth`. Tokens are 32 random bytes, base64url-encoded.

| method, path | auth | request | response |
|---|---|---|---|
| `POST /auth/register/start` | – | `username, registration_request` | `registration_response` |
| `POST /auth/register/finish` | – | `username, registration_upload, key_bundle, recovery_verifier, device_name` | `Session` |
| `POST /auth/login/start` | – | `username, credential_request` | `handshake_id, credential_response` |
| `POST /auth/login/finish` | – | `handshake_id, credential_finalization, device_name` | `Session + key_bundle` |
| `POST /auth/refresh` | – | `refresh_token` | `Tokens` |
| `GET /auth/key-bundle` | bearer | – | `key_bundle` |
| `POST /auth/logout` | bearer | – | `204` |
| `GET /auth/sessions` | bearer | – | `[{id, device_name, created_at, last_used_at, current}]` |
| `DELETE /auth/sessions/{id}` | bearer | – | `204` / `404` |
| `POST /auth/passphrase/start` | bearer | `credential_request, registration_request` | `handshake_id, credential_response, registration_response` |
| `POST /auth/passphrase/finish` | bearer | `handshake_id, credential_finalization, registration_upload, key_bundle` | `204` |
| `POST /auth/recover/start` | – | `username, recovery_verifier, registration_request` | `handshake_id, key_bundle, registration_response` |
| `POST /auth/recover/finish` | – | `handshake_id, registration_upload, key_bundle, device_name` | `Session` |

The key-bundle fetch lets a signed-in native client notice a passphrase change made elsewhere: it compares a fingerprint (for example SHA-256) of the fetched bytes with the bundle it holds, and if they differ, asks for the new passphrase. (A passphrase change already revokes other sessions, so in practice the device sees `session_ended` first; the fetch covers the device's "am I still current" check after re-login.)

`Session` = `{session_id, access_token, access_expires_in, refresh_token, username, is_admin}`; login adds `key_bundle`. Login, register finish, and recover finish also take an optional `session_kind` (`native` default, or `browser`). The sessions list includes `session_kind`. The vault ID is inside the bundle. Timestamps are Unix seconds.

OPAQUE registration is stateless on the server (the response is a function of the setup and credential identifier), so register and recover only need pending state for what the server has verified, not for OPAQUE itself.

Request bodies are well under the 1 MiB default limit; no per-route override.

### Client state machines (core)

Sans-I/O: each step takes the server's response bytes and returns the next request plus a new state. Randomness comes from `OsRng` inside core, like the rest of `crypto`.

- `Registration::start(username, passphrase) -> (Registration, RegisterStartRequest)`; `.finish(response, device_name) -> (RegisterFinishRequest, NewAccount)`. `finish` generates `VaultId` (ULID from the current time, which the caller passes in, matching `note`'s time-injection style), `VaultKey`, `RecoveryKey`, builds the bundle with the export key, and derives the recovery verifier. `NewAccount` holds the `VaultKey` and the `RecoveryKey` for one-time display.
- `Login::start(username, passphrase)`; `.finish(response, device_name) -> (LoginFinishRequest, LoginKeys)`; `LoginKeys::unlock(key_bundle_bytes) -> VaultKey`. The `AuthSecret` stays inside `LoginKeys` and is zeroized on drop.
- `PassphraseChange::start(username, old, new)` runs a login and a registration together; `.finish(response, current_bundle)` unlocks with the old export key and calls `KeyBundle::change_auth(old, new)`.
- `Recovery::start(username, recovery_key, new_passphrase)` derives the verifier; `.finish(response)` calls `KeyBundle::recover_auth`.

The minimum passphrase length (12 characters, counted as Unicode scalar values, matching the desktop app's rule) is enforced in `start` for registration, passphrase change, and recovery. The server can't see the passphrase, so there is no server-side rule. Passphrases are taken as `&str`, NFC-normalized by the caller-facing API so the same passphrase typed on different platforms yields the same OPAQUE input; the normalized buffer is `Zeroizing`. (This adds `unicode-normalization`, which is small and wasm-safe.)

### Recovery verifier

`RecoveryKey::verifier() -> [u8; 32]` = `HKDF-SHA256(salt = none, ikm = the 16 random bytes, info = "notebook/v1/recovery-verifier")`. The wrapping key uses `info = "notebook/v1/kek/recovery"` with the vault ID as salt, so the two are independent PRF outputs. No salt is needed because each recovery key is unique 128-bit randomness; and the client does not know its vault ID before recovering, so the vault ID can't be used. The server stores `SHA-256(verifier)`; since the verifier has 128 bits of entropy, a fast hash is sufficient and a stolen database still doesn't let anyone start recovery.

**Alternative considered:** run recovery as a second OPAQUE registration keyed on the recovery key. Rejected: twice the records and protocol steps for no gain, since the recovery key is already high-entropy and doesn't need OPAQUE's guess-resistance.

For an unknown username, `recover/start` hashes the supplied verifier and compares against a fixed dummy value so timing doesn't depend on whether the user exists.

### Pending handshakes

`login/start`, `passphrase/start`, and `recover/start` store state in a `Mutex<HashMap<HandshakeId, Pending>>` with a 60-second TTL: the `ServerLogin` state plus the user ID it was for (or a marker for "unknown user", whose finish always fails). Finish removes the entry (single use). A sweep every 30 seconds drops expired entries; the map is capped at 10,000 entries, beyond which start returns `503`, so floods are bounded in memory (the rate limiters make reaching the cap hard). Recovery pending state records the verified user ID and expires after the same 60 seconds; the client's Argon2 work happens before `start`, so that's enough.

**Alternative considered:** serialize `ServerLogin` into an encrypted, MAC'd blob returned to the client, making the server stateless. Rejected for now: more crypto to get right, and the single-binary deployment already keeps in-memory state.

### Login without revealing existence

`ServerLogin::start` with `None` as the password file produces a fake response derived from the OPRF seed and the credential identifier, indistinguishable from a real one and stable across attempts. Finish fails with `invalid_credentials` exactly as a wrong passphrase does. Registration's `username_taken` does reveal existence, but registration is closed (or invite-gated) and `invite-codes` checks the invite first.

### Sessions and tokens

One `sessions` row per device login:

```
sessions(session_id BLOB PK, vault_id BLOB, user_id BLOB, device_name TEXT, kind TEXT,
         access_hash BLOB UNIQUE, access_expires_at INT,
         refresh_hash BLOB UNIQUE, refresh_expires_at INT,
         prev_refresh_hash BLOB UNIQUE NULL,
         created_at INT, last_used_at INT)
index (vault_id), index (user_id)
```

- Tokens are random 32-byte values; the database stores `SHA-256(token)` and lookups go by hash (an indexed equality on a hash is not a timing oracle for the token). Opaque random tokens over JWTs: revocation is immediate, nothing to sign or rotate, and every request already hits SQLite.
- Access: 15 minutes. Refresh: 30 days from issue for `native` sessions and 12 hours for `browser` sessions, renewed on each rotation (so a session stays alive while it is used). Constants, not config. The browser keeps its refresh token in memory only and doesn't log out on tab close, so the short lifetime bounds how long an abandoned tab's session lingers; the kind is a client hint, not a security boundary (a client claiming `native` only gets what any native client gets).
- Refresh rotates both tokens and moves the old refresh hash to `prev_refresh_hash`. A presented token matching some session's `prev_refresh_hash` deletes that session (reuse detection).
- Refresh failures caused by a missing session (revoked, logged out, expired, reuse-detected, or unknown token) return `401 session_ended`, distinct from `invalid_token` on access-token checks. Core's client exposes `RefreshOutcome::{Refreshed(Tokens), SessionEnded, Transient}` by classifying the response, so `sync-client-engine` can tell "log in again" apart from "retry later" and keep the refresh token on transient failure.
- Revocation, logout, and reuse delete the row. `last_used_at` updates on refresh and at most once a minute on access, to avoid a write per request.
- A background task deletes rows with `refresh_expires_at < now` hourly.
- `sessions::authenticate_token(pool, token: &str) -> Result<AuthSession, AuthError>` is the single access-token check: decode, hash, look up a live session whose access token is unexpired, and return `AuthSession { user_id, vault_id, session_id }` or `invalid_token`. The axum `AuthSession` extractor reads `Authorization: Bearer` and calls it, returning `401 invalid_token` on failure. `sync-server` uses the extractor on its HTTP routes. On the WebSocket it calls `authenticate_token` directly with the token from the first message, because browsers can't set an `Authorization` header on a WebSocket. `sessions::is_active(pool, session_id)` lets it re-check long-lived sockets.

Bearer tokens rather than cookies: the browser client keeps everything in memory anyway, native clients have no cookie jar to lean on, and it removes CSRF from consideration.

### Passphrase change and recovery writes

The finish handlers parse the uploaded bundle with `KeyBundle::from_bytes`, check `vault_id()` equals the stored vault and `same_recovery_entry(&stored)` (a new constant-time comparison accessor in core), then in one transaction replace `users.opaque_record` and `key_bundles.bundle`, and delete sessions (all others for passphrase change; all for recovery, then insert the new one). The server can parse the bundle because its layout is public; it still can't unwrap it.

### Registration admission (interim gating, design only)

The spec defines registration as a plain OPAQUE start/finish flow; it deliberately has no "registration is closed" requirement, because `invite-codes` adds the invite requirement as ADDED requirements and must not have to modify or remove one of ours. Until `invite-codes` lands, `AuthState` holds a `RegistrationPolicy` enum: `Closed` (what production builds) and `Open` (a test-only constructor). While `Closed`, both registration handlers return `403` with `{"error":"registration_closed"}` before parsing the username, so nothing is revealed. `invite-codes` replaces the policy with its invite check (read-only at start, consumed at finish). No config key is added, so nothing can open public sign-up by accident.

Account creation is one function, `store::create_account(tx, NewAccount)`, used by `register/finish` and by `user add`, so the CLI and HTTP paths can't drift. The `register/finish` handler opens the transaction with `BEGIN IMMEDIATE` (taking the write lock up front, so the username check and inserts can't interleave with another writer) and passes the open transaction through a single extension point before `create_account`, so `invite-codes` can consume the invite as the first statement of the same transaction. The request types carry no invite field here; `invite-codes` adds `invite_code` to both registration requests.

### Rate limiting

Reuse `KeyedLimiter` for per-address limits, attached with `route_layer`:
- `login/start`: burst 10, one per 6 s per address. Plus a per-username limiter (governor keyed on the normalized username string, burst 10, one per minute), checked in the handler after parsing the username and before OPAQUE work. Unknown usernames consume tokens the same way.
- `recover/start`: burst 5, one per 10 minutes per address.
- `register/start`: burst 5, one per minute per address.
- `passphrase/start`: per-username limiter shared with login, since it includes a login.
- Finish, refresh, sessions, and logout rely on the global limiter: finish needs a handshake ID from a limited start, and refresh tokens are unguessable.

`KeyedLimiter` gains a small generic sibling for string keys (or is made generic over the key) so the retain task covers both. The per-username limiter lets an attacker slow one user's logins (a soft lockout); accepted, since it never blocks for longer than the refill interval and doesn't reveal anything.

### Schema

```
0001_accounts.sql
users(user_id BLOB PK, username TEXT UNIQUE NOT NULL, opaque_record BLOB NOT NULL,
      server_key_id INT NOT NULL DEFAULT 1, recovery_verifier_hash BLOB NOT NULL,
      is_admin INT NOT NULL DEFAULT 0, created_at INT, updated_at INT)
vaults(vault_id BLOB PK, user_id BLOB UNIQUE NOT NULL REFERENCES users, created_at INT)
key_bundles(vault_id BLOB PK REFERENCES vaults, bundle BLOB NOT NULL, updated_at INT)
sessions(... as above, REFERENCES users, vaults ON DELETE CASCADE)
server_meta(key TEXT PK, value BLOB NOT NULL)
```

User IDs are server-generated ULIDs (16-byte blobs). One vault per user, enforced by the unique `user_id`. `users` has no `vault_id`: it is account data, not vault data, and the vault is reached through `vaults.user_id`; every vault-scoped table (`key_bundles`, `sessions`, and later `ops`/`snapshots`) follows the convention. The username's `UNIQUE` constraint is what makes the concurrent-registration race safe: the second insert fails and is mapped to `username_taken`.

### Operator CLI

```
notebook-server user add <username> [--admin]
```

Creates the complete account (user, vault, key bundle, recovery verifier), so a client's first sign-in is an ordinary login and the recovery key is shown only by the CLI. The alternative, a vault-less account whose first client sign-in creates the bundle, was rejected: it needs a second "first login" flow in every client and a window where the account exists without a recovery path. Loads config, migrates, loads or creates the key file (same rules as `serve`), prompts twice with `rpassword` (no echo; refuses when stdin is not a terminal unless `--passphrase-stdin` reads one line, for scripted setups and tests), runs `Registration` in-process against `ServerRegistration` directly (no HTTP), calls `store::create_account`, and prints the recovery key with a warning that it is shown once. `invite-codes` adds `user quota` beside it under the same `user` subcommand group.

### Logging

The existing trace layer logs only method, path, status, and latency, so tokens and OPAQUE messages never reach logs. Handlers log auth failures at `debug` with the reason category and no username (usernames are personal data and would turn logs into an existence oracle for anyone reading them).

## Risks / Trade-offs

- [`opaque-ke` major versions change APIs and wire formats] → Pin the exact version; a recorded-transcript test (fixed RNG seed through registration and login) fails if a dependency bump changes bytes, since that would invalidate stored records.
- [Multiple `rand_core`/`getrandom` versions: opaque-ke's `rand_core` may differ from core's `getrandom` 0.4, and each `getrandom` version needs its own wasm backend feature] → Bridge with a small `CryptoRng` adapter over `getrandom` 0.4 if the versions differ; the wasm32 CI build catches a missing backend.
- [Argon2 64 MiB stretching on every login is slow on low-end phones and in the browser] → One-off cost per login; matches `vault-crypto`'s parameters. Native apps unlock from the keychain afterwards without re-running it.
- [Refresh reuse detection signs out a client whose refresh response was lost in transit] → Rare; the user logs in again and nothing is lost. A grace window would need the server to keep the plaintext of the newest token, which it shouldn't.
- [Key file lost] → Startup refuses rather than silently regenerating; deployment docs cover backup; worst case every user recovers with their recovery key, and no notes are lost.
- [Database and key file stolen together] → Offline guessing becomes possible, slowed by Argon2. The docs say to keep the key file out of the Litestream bucket.
- [In-memory pending handshakes and limiters reset on restart] → A login in flight during a restart fails and is retried; counters resetting is accepted (same as `server-foundation`).
- [Per-username limiter allows a soft lockout of a known username] → Bounded to the refill rate; per-address limits still apply to the attacker.
- [`recover/start` returns the key bundle to anyone with the verifier] → The verifier requires the recovery key, which can already unwrap the bundle; nothing new is exposed.
- [Usernames are ASCII-only] → Simple and unambiguous; can be widened later without breaking existing names.

## Migration Plan

Additive: one new migration and one new config key with a default. On the first start after upgrade, the server creates the key file (no users exist yet). Rollback before any accounts exist is replacing the binary and restoring the pre-upgrade database backup, as `docs/deployment.md` describes; after accounts exist, rolling back discards them.
