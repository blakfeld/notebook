# Spec Delta

## ADDED Requirements

### Requirement: Registration requires an invite code
Every registration start and finish request SHALL carry an invite code. The server SHALL validate the code before parsing or looking up the username or doing any OPAQUE work, and SHALL reject a missing or invalid code with `403` and `invalid_invite`, creating nothing.

#### Scenario: Valid invite
- **WHEN** a client registers with an unused, unexpired, unrevoked invite code and an available username
- **THEN** the account and its vault are created and the invite is marked as used by that account

#### Scenario: Missing invite
- **WHEN** a client sends a registration request without an invite code
- **THEN** the request is rejected with the invalid-invite error and no account is created

#### Scenario: Malformed username with invalid invite
- **WHEN** a client registers with an invalid invite code and a username that fails validation
- **THEN** the response is the invalid-invite error, not `invalid_username`

#### Scenario: Username is not revealed without a valid invite
- **WHEN** a client registers with an invalid invite code and a username that is already taken
- **THEN** the response is the invalid-invite error, identical to the one for an available username

### Requirement: Invite consumption is atomic with account creation
The invite SHALL be marked used in the same database transaction that creates the user, vault, key bundle, and recovery verifier. If account creation fails for any reason, the invite SHALL remain redeemable; if the invite cannot be consumed, no account SHALL be created.

#### Scenario: Username taken at finish
- **WHEN** a client completes registration with a valid invite but the username was taken by another registration in the meantime
- **THEN** the registration fails, no account is created, and the invite is still active

#### Scenario: Invite consumed concurrently
- **WHEN** a registration reaches its final step after the same code was consumed by another registration
- **THEN** no user, vault, or key bundle is created for it

### Requirement: Operator-created accounts need no invite
Accounts created by the operator with `notebook-server user add` SHALL NOT require or consume an invite code.

#### Scenario: Operator adds a user
- **WHEN** an operator creates an account with `user add` while no invites exist
- **THEN** the account is created normally and `invite list --all` is still empty

### Requirement: Vaults have a storage quota
Every vault SHALL have a storage quota in bytes. A new vault, whether created by registration or by `user add`, SHALL get the configured default quota (`default_vault_quota_bytes`, default 100 MiB). Vaults that exist when this capability is introduced SHALL be assigned the default quota.

#### Scenario: New vault gets the default
- **WHEN** the default quota is configured as 50 MiB and a user registers
- **THEN** the new vault's quota is 50 MiB

#### Scenario: Operator-created vault gets the default
- **WHEN** the default quota is configured as 50 MiB and an operator runs `user add`
- **THEN** the new vault's quota is 50 MiB and its usage is 0

#### Scenario: Default unchanged for existing vaults
- **WHEN** the operator changes the configured default quota and restarts the server
- **THEN** existing vaults keep the quota they already had

### Requirement: Vault storage usage
A vault's usage SHALL be the total size in bytes of the encrypted payloads the server stores for it: sync log updates and snapshots. Usage SHALL reflect every stored, compacted, and purged payload as soon as the write that caused it commits.

#### Scenario: Usage grows with pushes
- **WHEN** a client pushes an encrypted update of 2,000 bytes to an empty vault
- **THEN** the vault's usage is 2,000 bytes

#### Scenario: Usage shrinks on purge
- **WHEN** a trashed note is permanently purged
- **THEN** the vault's usage decreases by the size of that note's stored updates and snapshots

### Requirement: Writes beyond quota are rejected
A write that would leave a vault's usage above its quota SHALL be rejected with status `507` and a `quota_exceeded` error, storing nothing. A write that leaves usage no higher than before, such as a purge or a compacting snapshot, SHALL NOT be rejected for quota.

#### Scenario: Push over quota
- **WHEN** a vault is 100 bytes below its quota and a client pushes a 500-byte update
- **THEN** the push is rejected with `507` and `quota_exceeded`, and the update is not stored or relayed to other clients

#### Scenario: Compaction allowed at quota
- **WHEN** a vault is at its quota and a client uploads a snapshot whose compaction removes more bytes than the snapshot adds
- **THEN** the snapshot is accepted and usage decreases

#### Scenario: Live channel push over quota
- **WHEN** a client pushes an update over the live channel that would exceed the quota
- **THEN** the server answers that push with a `quota_exceeded` error and keeps the connection open

### Requirement: Operator manages vault quotas
`notebook-server user quota <username>` SHALL print the user's vault usage and quota. `notebook-server user quota <username> <size>` SHALL set the quota, accepting a byte count or a size with a binary unit (`KiB`, `MiB`, `GiB`). Setting a quota below current usage SHALL be allowed and SHALL block further growth only. An unknown username SHALL exit non-zero.

#### Scenario: Raise a quota
- **WHEN** an operator runs `notebook-server user quota alice 1GiB`
- **THEN** alice's vault quota becomes 1,073,741,824 bytes and the command exits 0

#### Scenario: Lower below usage
- **WHEN** an operator sets a quota smaller than the vault's current usage
- **THEN** the command succeeds, later pushes that grow the vault are rejected, and purges still succeed

#### Scenario: Unknown user
- **WHEN** an operator runs `user quota` for a username that does not exist
- **THEN** the command exits non-zero and nothing changes

## MODIFIED Requirements

### Requirement: Recovery and registration are rate limited
Recovery start requests SHALL be limited per client address (burst 5, refilling one every 10 minutes). Registration start and finish requests SHALL share one per-client-address limit whose burst and refill rate are configurable (`registration_rate_limit_burst`, default 5, and `registration_rate_limit_per_minute`, default 1). Over-limit requests SHALL get `429` with `Retry-After`.

#### Scenario: Recovery burst
- **WHEN** one client sends 6 recovery attempts in quick succession
- **THEN** the sixth gets `429`

#### Scenario: Registration start and finish share the limit
- **WHEN** the registration limit is at its defaults and one client sends 3 registration starts and then 3 registration finishes in quick succession
- **THEN** the sixth request gets `429`

#### Scenario: Configured registration limit
- **WHEN** `registration_rate_limit_burst` is set to 2 and one client sends 3 registration starts in quick succession
- **THEN** the third gets `429`

### Requirement: Auth errors are uniform JSON
Every error returned by an `/auth` endpoint handler (as opposed to the shared rate-limit, size, and timeout layers) SHALL be a JSON object with a single `error` field holding a snake_case code (`invalid_request`, `invalid_username`, `invalid_credentials`, `invalid_token`, `session_ended`, `username_taken`, `invalid_invite`), and SHALL NOT include internal details.

#### Scenario: Error body
- **WHEN** a login finish fails because of a wrong passphrase
- **THEN** the body is exactly `{"error":"invalid_credentials"}`

#### Scenario: Invalid invite body
- **WHEN** a registration start is rejected because its invite code is not valid
- **THEN** the body is exactly `{"error":"invalid_invite"}`
