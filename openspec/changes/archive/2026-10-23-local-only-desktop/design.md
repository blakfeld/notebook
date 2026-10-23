# Design

## Context

See proposal.md (Why). Requirements are in `specs/app-shell/spec.md` and `specs/vault-crypto/spec.md`.

Today `notebook-app` has a three-phase state machine (`Phase::SignedOut | Locked(Account) | Unlocked`) in `state.rs`. `vault.rs` opens `SqliteStorage` at `<data_dir>/vault.sqlite3`, builds a `Replica` and `Workspace`, and runs a `SyncEngine` with a `NativeTransport` and `AuthClient` on a dedicated thread. `credentials.rs` stores three keychain entries under `com.blakfeld.notebook`: `session` (server, username, vault ID, refresh token), `local-unlock` (the vault key under the passphrase via `notebook_core::crypto::LocalUnlock`), and `auto-unlock` (the raw vault key, present while not locked). `KeyBundle` (`crypto/bundle.rs`) wraps the vault key under an auth secret (kind 32) and under the recovery key (kind 33).

The frontend has `lib/account/*` (sign-in, register, recover, unlock, reauth, secure-store notice), `lib/state/sync.svelte.ts`, `shell/SyncIndicator.svelte`, and account/passphrase/lock/sign-out sections in `Settings.svelte`.

## Goals / Non-Goals

**Goals:**
- The app works end to end with no server, account, or network, and existing local data carries over.
- Smallest change to the app that delivers that. Code deletion is left to `retire-sync-server`.

**Non-Goals:**
- Deleting the server crate, `notebook-core::{auth, sync, protocol}`, or the storage outbox. The app stops calling them, and `retire-sync-server` removes them.
- Backups and restore (`vault-backup`). This change only makes sure the recovery entry exists for them.
- An app lock (OS biometric or password). Listed as later work.
- Showing or rotating the recovery key after creation.
- Removing a notebook from a device ("sign out" equivalent).

## Decisions

### 1. Vault key in the keychain, recovery entry in the database

The keychain gets one entry, `vault-key` (vault ID plus the raw vault key, base64, as `auto-unlock` does today). The SQLite store gets the serialized `RecoveryEntry` in a new single-row table `vault_meta`, with a schema version bump and an upgrade that creates the table. The recovery entry is not secret on its own: it opens only with a 128-bit recovery key, which is far stronger than any passphrase wrap. So it can live on disk and be copied into backups unchanged.

*Alternative:* keep a passphrase as a local unlock (today's `local-unlock`). Rejected: the user asked to remove logins, and a passphrase prompt on every launch is a login in all but name. *Alternative:* a random "device secret" in the keychain as the `KeyBundle` auth secret, so `KeyBundle` is reused unchanged. Rejected: it keeps a server-shaped structure (two entries, rewrap on passphrase change) for no gain.

### 2. `RecoveryEntry` reuses the bundle's recovery wrap

`crypto/recovery_entry.rs`: layout `version (1) || vault id (16) || envelope`. The envelope uses `RecoveryKey::wrapping_key(vault)` and the existing kind 33 (`KIND_VAULT_KEY_RECOVERY`), so it is byte-for-byte the same wrap as a key bundle's recovery entry, only without the auth entry. API: `RecoveryEntry::create(vault, &VaultKey, &RecoveryKey)`, `open(&RecoveryKey) -> VaultKey`, `vault_id()`, `to_bytes()`, `from_bytes()`. A fixed test vector pins the bytes.

### 3. App phases

```
Setup                 no vault on this device → welcome screen (create; restore arrives with vault-backup)
PendingRecoveryKey    notebook created or upgraded, key shown, not yet acknowledged
NeedsRecoveryKey      vault exists, no usable keychain entry → recovery-key prompt
NeedsOldPassphrase    upgrade: legacy local-unlock exists but no auto-unlock key
Open                  replica loaded, editor usable
```

`PhaseDto` is replaced to match these phases. `Open` holds `Replica`, `Workspace`, and `SqliteStorage` directly, with no engine thread. Edits are written straight to storage through the existing `Workspace`/`Replica` local-update path, which already persists before reporting done. The engine was only the pusher. The `Shared` replica wrapper stays, since the editor bridge and commands use it.

Startup decision, in order:
1. Keychain `vault-key` present and its vault matches the database → `Open`.
2. Legacy `session` entry present → upgrade (Decision 5).
3. Database present with a recovery entry, no keychain entry → `NeedsRecoveryKey`. Without a secure store, the prompt also shows the explanatory notice (the existing `SecureStoreNotice` copy, reworded).
4. No database → `Setup`.

### 4. Creating a notebook

`create_notebook` generates the vault ID, vault key, and recovery key in memory and enters `PendingRecoveryKey`, returning the key's display form once. Nothing is written yet. `acknowledge_recovery_key` creates the database with the recovery entry, stores `vault-key` when a secure store exists, and moves to `Open`. Quitting before acknowledgment leaves nothing behind, so the next launch shows `Setup` again, and no notebook can exist whose recovery key the user never confirmed.

### 5. Upgrade from a server-backed install

Triggered by a legacy `session` keychain entry:
- Vault key from `auto-unlock` if present. Otherwise enter `NeedsOldPassphrase` and open `local-unlock` with the entered passphrase (`LocalUnlock::open`, Argon2id, off the UI thread). Otherwise, with no secure store, there is nothing to recover from.
- Generate a new recovery key and enter `PendingRecoveryKey`. On acknowledgment, open the existing database for that vault, write the `RecoveryEntry` to `vault_meta`, store `vault-key`, and only then delete `session`, `local-unlock`, and `auto-unlock`. Quitting before acknowledgment leaves the legacy entries in place, so the upgrade simply runs again with a fresh key.
- Outbox entries, the sync cursor, and acknowledged sequence numbers are left in the database untouched. They are harmless and `retire-sync-server` drops them in its schema upgrade.
- "Start over": rename `vault.sqlite3` to `vault.sqlite3.legacy-<timestamp>`, never delete it, then `Setup`.

A new recovery key is generated because the old one sits only in the server's key bundle, and the server is going away. The old recovery key stops mattering, which the acknowledgment copy says.

`LocalUnlock` and the Argon2id KDF stay in core for this path. They are migration-only code, kept until a later cleanup decides the upgrade window is over.

### 6. No network

`vault.rs` stops constructing `NativeAuthHttp`, `NativeTransport`, and `SyncEngine`. The CSP becomes `connect-src 'self' ipc: http://ipc.localhost` (dev keeps `ws:` for Vite HMR). `notebook-app` still depends on `notebook-core` with `transport-native` until `retire-sync-server` removes it. Dropping the feature here would break nothing at runtime but would churn `Cargo.toml` twice.

### 7. Frontend

- `lib/account/` is replaced by `lib/setup/`: `Welcome.svelte` (create notebook, plus a restore button slot that `vault-backup` fills), `RecoveryKeyShown.svelte` (reusing the registration screen's key display and acknowledgment checkbox), `RecoveryKeyPrompt.svelte` (reusing `Recover.svelte`'s forgiving key input), and `OldPassphrase.svelte`.
- `lib/state/sync.svelte.ts` and `shell/SyncIndicator.svelte` are deleted. The sidebar footer keeps its slot for `vault-backup`'s status.
- `Settings.svelte` drops the account, passphrase, lock, and sign-out sections, and gains an "About" section with the version.
- `platform/types.ts`, `core.ts`, `tauri.ts`, `fake.ts`, and `demo.ts` lose the account and sync methods and gain the new ones. The demo core starts in `Open` with seed notes.

## Risks / Trade-offs

- [Anyone with the user's OS session can open the notes, since there's no app lock] → Same as most local note apps. Disk-level ciphertext still protects copies of the data directory and backups. An OS-authenticated lock is later work.
- [User loses the recovery key and the keychain entry (new machine, OS reinstall)] → Backups can't be opened, and the acknowledgment screen says so plainly. Markdown export gives a plaintext escape hatch.
- [Upgrade on a device whose server held notes never downloaded] → Not possible in practice: sign-in bootstraps the full vault before the main layout shows. Upgrade notes in the README recommend letting sync settle first.
- [Storage logs grow without compaction until `retire-sync-server` lands] → Snapshot compaction ran inside the sync engine and is blocked by unacknowledged outbox entries. Fine for the short gap between the two changes at personal scale.
- [Keychain prompts on macOS for an unsigned dev build] → Unchanged from today.

## Migration Plan

One-way conversion on first launch of the new version (Decision 5). Nothing on disk changes before the user acknowledges the new recovery key. After that, the database is at layout version 2, which an older build refuses to open (`SchemaTooNew`), and the `session` entry is gone. So rolling back means signing in again on the old build, which downloads the vault from the server while it still runs. No server-side steps: the server is simply no longer used.

Launch never moves or deletes anything because a read failed: when the keychain or the database can't be read (a denied keychain prompt, a busy or damaged file), the app shows the recovery-key prompt and tries again at the next launch. A database is moved aside automatically only when the keychain answers that it holds no key for it. Legacy entries are deleted only when an upgrade is acknowledged, never by "Start over", so a passphrase remembered later still opens the moved file. "Start over" is offered from the old-passphrase screen, from the recovery-key prompt, and on an upgrade's recovery-key screen once acknowledging fails (its database is first opened then).

Known limitation: the keychain has one `vault-key` slot. If a notebook is moved aside by "Start over" while its `vault-key` entry is still in the keychain (because the keychain couldn't be read, for example after a denied prompt), creating the new notebook overwrites that entry, and the moved file then opens only with its recovery key.
