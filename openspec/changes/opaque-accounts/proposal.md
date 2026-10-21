# Proposal

## Why

A publicly hosted server needs accounts, but with end-to-end encryption the account layer can be very thin. There's no email, and a password-reset flow couldn't recover encrypted data anyway. Using OPAQUE means the server never sees the passphrase, and a stolen database alone gives an attacker nothing to test guesses against offline (the OPAQUE server key is kept outside the database). Login also gives the client a secret for unwrapping the vault key, so a single passphrase both authenticates the user and unlocks the vault.

## What Changes

- OPAQUE registration and login (`opaque-ke`), implemented once: the client side in `notebook-core` (wasm-compatible, no I/O) and the server side in `notebook-server`, sharing wire types from `notebook-core`.
  - Registration creates a user (username plus OPAQUE record), a vault, and stores the client-produced **encrypted key bundle** (vault key wrapped with the OPAQUE export key and, separately, with the recovery key), plus a **recovery verifier** derived from the recovery key.
  - Login yields a device session and returns the encrypted key bundle. The client unwraps it locally with the OPAQUE export key.
- Accounts have no email. Usernames are unique, case-insensitive ASCII, and login and recovery responses don't reveal whether a username exists.
- Sessions: short-lived access tokens plus rotating, revocable refresh tokens per device (all stored hashed). Session listing, logout, and per-device revocation. An authenticated-request extractor for later endpoints (`sync-server`).
- Recovery: a user proves possession of the recovery key, unwraps the vault key, and sets a new passphrase (re-registering OPAQUE and re-wrapping the bundle). All existing sessions are revoked.
- Passphrase change requires the current passphrase, re-wraps the key bundle, and revokes the user's other sessions; note keys and content are untouched. The server refuses any bundle update that would alter the recovery entry or vault ID.
- Rate limiting on login (per client address and per username), recovery, and registration endpoints.
- `users` table includes an `is_admin` flag (unused in UI for now).
- The OPAQUE server key pair lives in a separate key file, created on first start, with its fingerprint recorded in the database so a missing or wrong key file is detected rather than silently replaced.
- Registration exists here but HTTP registration is closed until invite codes gate it (see `invite-codes`). Until then, accounts are created only via the operator CLI (`notebook-server user add`).

## Capabilities

### New Capabilities
- `accounts`: User registration and login via OPAQUE, usernames, session lifecycle, server-stored encrypted key bundle, passphrase change, recovery-key account recovery, auth rate limits, the OPAQUE server key, and the `user add` operator command.

### Modified Capabilities
- `vault-crypto`: adds a recovery verifier, a value derived from the recovery key that lets the server check possession of the recovery key without learning anything that unwraps the vault key. Existing requirements are unchanged.

## Impact

- `notebook-core`: `auth` module with client-side OPAQUE state machines, username normalization, shared request/response types, and key-bundle handling (wasm-compatible); a recovery-verifier derivation in `crypto`.
- `notebook-server`: `/auth/*` endpoints; `users`, `vaults`, `key_bundles`, `sessions`, and `server_meta` tables; an OPAQUE key file and config key; a `user add` CLI command; deployment docs for backing up the key file.
- Dependencies: `opaque-ke` (ristretto255, Argon2 key stretching), `serde` and `base64` in core, `rpassword` for the CLI prompt.
- Consumed by `sync-server` (session authentication), `invite-codes` (registration gating), `sync-client-engine` (refresh), and the app shells (login, recovery, passphrase change UI).
