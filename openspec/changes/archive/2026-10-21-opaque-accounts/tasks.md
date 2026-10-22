# Tasks

## 1. Dependencies

- [x] 1.1 Add `opaque-ke` (exact pinned version, ristretto255; no `argon2` feature), `serde` (derive), `base64`, and `unicode-normalization` to the workspace and `notebook-core`; add `opaque-ke`, `rpassword`, and `ulid` to `notebook-server`; resolve the `rand_core`/`getrandom` versions (add a `CryptoRng` adapter over `getrandom` 0.4 if opaque-ke's `rand_core` differs, and enable any wasm backend feature a second `getrandom` needs); verify `cargo build --workspace` and `cargo build -p notebook-core --target wasm32-unknown-unknown` both succeed

## 2. Core crypto additions

- [x] 2.1 Add `RecoveryKey::verifier()` (HKDF-SHA256, no salt, info `notebook/v1/recovery-verifier`, 32 bytes); verify unit tests that the generated key and its re-parsed display form give the same verifier, two keys differ, the verifier used as an `AuthSecret` fails to unlock the bundle, and a fixed recovery key matches a recorded test vector
- [x] 2.2 Add `KeyBundle::same_recovery_entry(&other)` with constant-time comparison; verify unit tests that a bundle after `change_auth`/`recover_auth` matches the original and one created with a different recovery key does not

## 3. Core auth module (client side)

- [x] 3.1 Implement `auth::username::Username` (lowercase normalization, `[a-z0-9][a-z0-9._-]{2,31}`); verify unit tests for case folding, length 2/3/32/33, a space, a non-ASCII letter, and a leading `.`
- [x] 3.2 Implement `auth::suite` (ristretto255, TripleDH/SHA-512, Argon2id KSF wrapper using `KdfVersion::V1` parameters via the workspace `argon2`); verify a test that a client and server built from the suite complete registration and login in-process and the client's export key is 64 bytes
- [x] 3.3 Implement `auth::api` request/response types for every route in the design's table (base64url-no-pad byte fields, `session_kind`, `RefreshOutcome`, error body); verify serde round-trip tests and a test that an unknown error code or malformed base64 fails to parse rather than panicking
- [x] 3.4 Implement `Registration` (passphrase ≥ 12 scalar values after NFC, vault ID from caller-supplied time, vault key, recovery key, bundle under the export key, recovery verifier; `NewAccount` returns the recovery key once) and `Login` (`LoginKeys::unlock`); verify in-process tests against `opaque-ke` server types: register then login unlocks a vault key that unwraps a note key wrapped at registration, a wrong passphrase fails, an 11-character passphrase is refused before any message is produced, NFC and NFD forms of the same passphrase both log in, and the serialized requests contain neither the passphrase marker nor the recovery key in any encoding
- [x] 3.5 Implement `PassphraseChange` and `Recovery` client flows; verify in-process tests that the new passphrase unlocks the same vault key, the old one fails, the recovery key still unlocks, and the uploaded bundle passes `same_recovery_entry` against the original
- [x] 3.6 Implement refresh response classification into `RefreshOutcome::{Refreshed, SessionEnded, Transient}`; verify unit tests that `401 session_ended` is `SessionEnded` and `429`, `503`, and a missing response are `Transient`
- [x] 3.7 Verify `cargo build -p notebook-core --target wasm32-unknown-unknown` succeeds with the auth module and that debug output of `LoginKeys`, `NewAccount`, and the state machines contains no key or passphrase bytes (unit test)

## 4. Server schema and key file

- [x] 4.1 Add `migrations/0001_accounts.sql` with `users`, `vaults`, `key_bundles`, `sessions` (including `kind`), `refresh_history`,, and `server_meta` as in the design, with foreign keys and `vault_id`/`user_id` indexes; verify the existing migration tests still pass and a new test checks the tables exist and the username `UNIQUE` constraint rejects a duplicate
- [x] 4.2 Add the `opaque_key_path` config key (default `opaque-server.key`) with env override; verify config unit tests for default, file, and `NOTEBOOK_OPAQUE_KEY_PATH`
- [x] 4.3 Implement `auth::keyfile` (create-new with mode `0600` only when no users exist, load, fingerprint record/compare in `server_meta`, errors naming `opaque_key_path`) and call it from `serve`; verify tests for first start (file created, owner-only permissions, fingerprint stored), restart with the same file, missing file with existing users (refuses, no file created), and a key file from another deployment (refuses with the mismatch error)

## 5. Server sessions

- [x] 5.1 Implement `auth::store` (create account in a passed-in transaction, lookups by normalized username, bundle read/replace, record replace) and `auth::sessions` (issue tokens as SHA-256 hashes, refresh rotation with `refresh_history` reuse detection (any earlier token), 15-minute access, 30-day native / 12-hour browser refresh, revoke, list, throttled `last_used_at`, `is_active`); verify unit tests against a temp database for rotation, reuse revoking the session, both expiry lengths, and that the stored rows contain no raw token bytes
- [x] 5.2 Implement `sessions::authenticate_token(&str)` (the one access-token check, public for `sync-server`'s WebSocket first-message auth) and the `AuthSession` extractor built on it (`Authorization: Bearer`, `401 invalid_token`) and the hourly expired-session cleanup task; verify unit tests calling `authenticate_token` directly for valid, malformed, expired, and revoked tokens, router tests with a test-only protected route for the same cases plus a missing header, and a test that runs the cleanup function and finds an expired session deleted

## 6. Server auth endpoints

- [x] 6.1 Implement `AuthState` (server setup, pending-handshake TTL map with 60 s expiry, 10,000-entry cap returning `503`, and 30 s sweep, `RegistrationPolicy` with `Closed` in production and a test-only `Open`) and mount `auth::routes()` in `app::router()`; verify unit tests for pending-map expiry, single use, and the cap
- [x] 6.2 Implement `register/start` and `register/finish` (policy check first, `BEGIN IMMEDIATE` transaction with a single extension point before `create_account`, bundle parse and vault-ID uniqueness, `username_taken` on conflict, device name ≤ 64, session kind); verify router tests driving the core client: successful registration then login returns the same bundle bytes, malformed bundle creates nothing, duplicate username, two concurrent finishes for one username leave exactly one account, and the `Closed` policy returns `403 registration_closed` identically for taken and unused usernames
- [x] 6.3 Implement `login/start` and `login/finish` (fake response for unknown users, `invalid_credentials` for wrong passphrase, unknown user, expired or replayed handshake; returns session, username, `is_admin`, and bundle); verify router tests for success, wrong passphrase, unknown username (same response size, stable server-derived fields across two starts, same finish error), a finish after 60 s (using a paused clock), and a replayed finish creating no second session
- [x] 6.4 Implement `refresh`, `logout`, `GET /auth/sessions`, and `DELETE /auth/sessions/{id}`; verify router tests: refresh invalidates the previous access token, reuse ends the session with `session_ended`, a revoked device's refresh gets `session_ended`, logout invalidates both tokens, listing shows device name, kind, and the current flag, revoking another user's session ID returns `404` and leaves it working, and no response in the full flow carries `Set-Cookie`
- [x] 6.5 Implement `GET /auth/key-bundle` (bearer); verify router tests that it returns the bytes returned at login, changes after a passphrase change, and needs a valid access token
- [x] 6.6 Implement `passphrase/start` and `passphrase/finish` (login with the current passphrase plus re-registration; vault ID and recovery-entry checks; atomic record and bundle replace; revoke other sessions); verify router tests for success (new passphrase works, old fails, recovery key still unlocks, other session ends, current continues), wrong current passphrase leaving data unchanged, a bundle with an altered recovery entry, and a bundle naming another vault
- [x] 6.7 Implement `recover/start` and `recover/finish` (verifier hash constant-time comparison, dummy comparison for unknown users, same bundle checks, revoke all sessions, return a new session); verify router tests for successful recovery (new passphrase works, old fails, every earlier session ends), recovering twice with the same key, wrong recovery key, and unknown username returning the identical `invalid_credentials` body
- [x] 6.8 Map all handler errors to `{"error": "<code>"}` with no internal details; verify a test asserting the exact body `{"error":"invalid_credentials"}` for a wrong passphrase, and a test that the database file contains none of the issued tokens, the recovery verifier, or the passphrase marker after a full register/login/refresh flow

## 7. Auth rate limits

- [x] 7.1 Make the limiter usable with string keys (generic `KeyedLimiter` or a sibling covered by the retain task) and attach per-address limiters to `login/start` (10, 1/6 s), `recover/start` (5, 1/10 min), and `register/start` (5, 1/min), plus the per-username limiter (10, 1/min) checked in `login/start` and `passphrase/start` before OPAQUE work; verify router tests with `MockConnectInfo` that 11 login attempts for one username from different addresses get `429` with `Retry-After`, an unknown username is limited the same, a different username from another address still logs in, and the sixth recovery attempt gets `429`

## 8. Operator CLI and docs

- [x] 8.1 Add `notebook-server user add <username> [--admin] [--passphrase-stdin]` (load config, migrate, load or create the key file, prompt twice without echo or read one line from stdin, run the core `Registration` against `ServerRegistration` in-process, `create_account`, print the recovery key once); verify CLI parse tests and an integration test that adds a user via `--passphrase-stdin`, then logs in over the router (receiving the existing bundle, which unlocks), checks `is_admin` for `--admin`, and that an invalid name, a taken name, mismatched prompts (unit-tested prompt logic), and a short passphrase exit non-zero with nothing created
- [x] 8.2 Update `docs/deployment.md` (the key file, its permissions, backing it up separately from the Litestream replica, what losing it means, creating the first account with `user add`) and the README server section; verify the sample config in the doc still migrates and that `notebook-server user add --help` matches the documented usage

## 9. Integration checks

- [x] 9.1 Add an end-to-end router test: `user add` creates an account; the client logs in on two devices (one `browser`), refreshes, changes passphrase from one (the other is signed out), then recovers with the recovery key and logs in with the newest passphrase, unwrapping the same note key throughout; verify it passes
- [x] 9.2 Run `scripts/check.sh` and verify it exits 0 (fmt, clippy with `-D warnings`, workspace tests, wasm32 core build)
- [x] 9.3 Run `openspec validate opaque-accounts --strict` and verify it passes

## Workflow follow-up

- Review the change with the code-reviewer and security-analyst agents (this touches crypto, auth, and secret storage), then archive it with `/opsx:archive` so `accounts` lands in `openspec/specs/` before `invite-codes` and `sync-server` are archived.
- Update `TODO.md`: mark `opaque-accounts` as `[x]` once archived.
