# Proposal

## Why

The server is publicly hosted but shouldn't allow open sign-up: strangers could create accounts and fill the disk. Invite codes let the operator decide who joins, with minimal machinery. Pairing them with per-vault storage quotas bounds what any single account can consume.

## What Changes

- Operator CLI: `notebook-server invite create [--expires <duration>] [--note <text>]` prints a single-use code once. `invite list` and `invite revoke` manage outstanding codes.
- Codes are high-entropy, stored only as hashes, single-use, and expire (default 7 days).
- Registration requires a valid invite code. The code is consumed atomically in the same transaction that creates the account, so concurrent use of one code can't create two accounts.
- Invite redemption attempts are rate-limited separately from login. The existing registration limit from `opaque-accounts` is made configurable and also covers registration finish, so codes can't be guessed through either step.
- The invite records who created it and who used it.
- Public registration is opened, gated by the invite: the interim `RegistrationPolicy` (always closed in production) is replaced by the invite check. The code is checked before any username or OPAQUE work, and all invalid-code cases get the same error. Operator-created accounts (`user add`) need no invite.
- Per-vault storage quotas: new vaults get a configurable default quota (100 MiB). Usage is the total size of the vault's stored encrypted updates and snapshots. Writes that would exceed the quota are rejected with `507` and a `quota_exceeded` error; deletes, purges, and compacting snapshots are always allowed. The operator can view and set a user's quota with `notebook-server user quota`.

## Capabilities

### New Capabilities
- `invites`: Invite code creation, listing, revocation, expiry, single-use redemption, and rate limiting.

### Modified Capabilities
- `accounts` (introduced by `opaque-accounts`): registration now requires a valid invite code, new vaults (including those created by `user add`) get a storage quota, the registration rate limit becomes configurable and covers finish, and `invalid_invite` joins the auth error codes.

## Impact

- `notebook-server`: `invites` table, `vaults.quota_bytes` / `used_bytes`, a change to the registration endpoint, and new CLI subcommands.
- Clients: registration screens gain an invite-code field (implemented in app-shell and web-client changes).
- Quota enforcement is wired into the `sync-server` write paths (push, snapshot upload, purge), which exist by the time this change lands.
- `notebook-core`: registration request types in `auth::api` gain an `invite_code` field; shared error codes gain `invalid_invite` and `quota_exceeded`.
