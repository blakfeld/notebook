# Proposal

## Why

A publicly hosted server needs accounts, but with end-to-end encryption the account layer can be very thin. There's no email, and a password-reset flow couldn't recover encrypted data anyway. Using OPAQUE means the server never sees the passphrase or anything that could be used to test guesses offline, even if the database is stolen. Login also gives the client a secret for unwrapping the vault key, so a single passphrase both authenticates the user and unlocks the vault.

## What Changes

- OPAQUE registration and login (`opaque-ke`), implemented once in `notebook-core` (client side) and `notebook-server` (server side):
  - Registration creates a user (username plus OPAQUE record) and a vault, and stores the client-produced **encrypted key bundle** (vault key wrapped with the OPAQUE export key and, separately, with the recovery key).
  - Login yields a session token and returns the encrypted key bundle. The client unwraps it locally with the OPAQUE export key.
- Accounts have no email. Usernames are unique, and responses don't reveal whether a username exists.
- Sessions: short-lived access tokens plus revocable refresh tokens per device (stored hashed). Logout and per-device revocation.
- Recovery: a user can log in with the recovery key to unwrap the vault key and set a new passphrase (re-registering OPAQUE and re-wrapping the bundle).
- Passphrase change re-wraps the key bundle; note keys and content are untouched.
- Rate limiting on login and recovery endpoints.
- `users` table includes an `is_admin` flag (unused in UI for now).
- Registration exists here but isn't publicly reachable until invite codes gate it (see `invite-codes`). Until then, accounts are created only via the operator CLI.

## Capabilities

### New Capabilities
- `accounts`: User registration and login via OPAQUE, session lifecycle, server-stored encrypted key bundle, passphrase change, and recovery-key account recovery.

### Modified Capabilities
<!-- None. Builds on `vault-crypto` (introduced by `vault-encryption`) and `server-hosting` (introduced by `server-foundation`) without changing their requirements. -->

## Impact

- `notebook-core`: client-side OPAQUE flows and key-bundle handling (wasm-compatible).
- `notebook-server`: `/auth/*` endpoints and the `users`, `vaults`, `key_bundles`, `sessions` tables, plus a CLI `user add` command.
- Dependency: `opaque-ke`. OPAQUE needs a long-lived server key pair, so the design must cover its storage and rotation.
