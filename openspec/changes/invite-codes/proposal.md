# Proposal

## Why

The server is publicly hosted but shouldn't allow open sign-up: strangers could create accounts and fill the disk. Invite codes let the operator decide who joins, with minimal machinery. Pairing them with per-vault storage quotas bounds what any single account can consume.

## What Changes

- Operator CLI: `notebook-server invite create [--expires <duration>]` prints a single-use code once. `invite list` and `invite revoke` manage outstanding codes.
- Codes are high-entropy, stored only as hashes, single-use, and expire (default 7 days).
- Registration requires a valid invite code. The code is consumed atomically in the same transaction that creates the account, so concurrent use of one code can't create two accounts.
- Invite redemption attempts are rate-limited separately from login, so codes can't be guessed.
- The invite records who created it and who used it.
- Per-vault storage quotas: new vaults get a configurable default quota. Writes beyond quota are rejected with a clear error. The operator can adjust a user's quota from the CLI.

## Capabilities

### New Capabilities
- `invites`: Invite code creation, listing, revocation, expiry, single-use redemption, and rate limiting.

### Modified Capabilities
- `accounts` (introduced by `opaque-accounts`): registration now requires a valid invite code, and new vaults are assigned a storage quota.

## Impact

- `notebook-server`: `invites` table, `vaults.quota_bytes` / `used_bytes`, a change to the registration endpoint, and new CLI subcommands.
- Clients: registration screens gain an invite-code field (implemented in app-shell and web-client changes).
- Quota enforcement is consumed by `sync-server`.
