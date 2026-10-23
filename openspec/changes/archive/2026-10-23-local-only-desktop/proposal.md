# Proposal

## Why

Notebook is pivoting from a synced, multi-platform app (desktop, browser, mobile, plus a self-hosted sync server) to a single-user desktop app that keeps everything on the device. The server, accounts, and sign-in exist only to serve sync, and they are most of the operational and security surface. Without them the app opens straight into the user's notes, needs no network, and has nothing to run or keep patched. Off-device safety comes from encrypted backups to a folder the user picks, such as Dropbox (`vault-backup`), not from a server.

## What Changes

- **BREAKING**: The desktop app no longer signs in, registers, recovers an account, changes a passphrase, locks, or signs out. There are no user accounts or logins.
- On first launch the app creates a local notebook: a fresh vault ID, vault key, and recovery key. It shows the recovery key once and requires the user to confirm they saved it. The recovery key is what opens a backup on another computer.
- The vault key is kept in the OS secure credential store, so later launches open the notes with no prompt. Without a secure store, the app asks for the recovery key at each launch rather than writing the key to a file.
- A **recovery entry** (the vault key wrapped under the recovery key, bound to the vault ID) is stored with the local vault and written into every backup.
- The app stops starting the sync engine and makes no network requests. The CSP keeps only `'self'` and IPC.
- Permanent delete works immediately and offline. The sync status indicator is removed. Settings drop the server, account, passphrase, lock, and sign-out entries.
- **Upgrade path** for installs that were signed in to a server: on first launch of the new version the app opens the existing local vault once. It uses the stored vault key, or else the old passphrase through the existing device-local unlock. It then generates a recovery key, shows it once, stores the vault key under the new scheme, and deletes the old session and unlock entries. Notes are kept.
- The server crate, auth and sync code, and their specs are left in place and are deleted by `retire-sync-server`, so this change stays focused on the app's behavior.

## Capabilities

### New Capabilities
<!-- none -->

### Modified Capabilities
- `app-shell`: removes server address, sign-in, registration, account recovery, passphrase change, sign-out, lock and unlock, session expiry, and sync status. Adds first-launch notebook creation, opening on launch from the secure store, recovery-key entry when no secure store exists, upgrade from a server-backed install, and no network access. Rewrites recovery key display, delete, live updates, durability, tag rename, and settings without sync.
- `vault-crypto`: adds the recovery entry, a standalone, serializable wrap of the vault key under the recovery key that needs no auth secret.

## Impact

- `crates/notebook-app`: `Phase` becomes `Setup` (no vault yet), `NeedsRecoveryKey` (no secure store, or upgrade needing input), and `Open`. Commands for sign-in, register, recover, lock, unlock, reauthenticate, change passphrase, sign out, pending count, and sync status are removed. New commands: `create_notebook`, `acknowledge_recovery_key`, `open_with_recovery_key`, `upgrade_with_passphrase`. `credentials.rs` gets a single `vault-key` entry. `vault.rs` no longer builds a sync engine or auth client.
- `crates/notebook-core`: a `RecoveryEntry` type in `crypto` beside `KeyBundle`.
- `crates/notebook-storage-sqlite`: the recovery entry is stored with the vault (one new table or row, schema version bump).
- `web/`: `lib/account/` is replaced by a first-launch screen and a recovery-key prompt. `SyncIndicator.svelte` and the account parts of `Settings.svelte` are removed, and `platform/` loses the matching adapter methods.
- `tauri.conf.json` CSP: `connect-src` is narrowed to IPC only.
- Docs: `README.md` "Running" drops the server steps (the server code itself goes in `retire-sync-server`).
