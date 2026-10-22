# Tasks

## 1. Reconcile with landed changes

- [x] 1.1 Confirm `opaque-accounts` and `sync-server` are archived and list which registration and sync write functions, and any quota columns or stub, already exist; verify by reading `openspec/specs/accounts/spec.md` and `openspec/specs/sync-protocol/spec.md` and confirming no requirement there conflicts with this change's `specs/accounts/spec.md` (amend this change's delta first if one does)

## 2. Schema and configuration

- [x] 2.1 Add a migration creating `invites` (ULID `id`, unique `code_hash`, `note`, `created_at`, `expires_at`, `revoked_at`, `used_at`, `used_by`) and adding `vaults.quota_bytes` (default 104857600) and `vaults.used_bytes`, skipping columns that already exist; verify with a test that migrates a fresh database and asserts the columns and unique index
- [x] 2.2 Backfill `used_bytes` in the same migration from the summed payload lengths of each vault's `ops` and `snapshots`; verify with a test that seeds rows under the previous schema version, migrates, and checks the sums
- [x] 2.3 Add `default_vault_quota_bytes`, `registration_rate_limit_burst` (default 5), and `registration_rate_limit_per_minute` (default 1) to `Config` (file and `NOTEBOOK_` env, non-zero, documented defaults); verify with config unit tests for defaults, env override, and an unparseable value naming the key
- [x] 2.4 Document the three keys in the sample `notebook.toml` in `docs/deployment.md`; verify the sample still loads in the existing config-file test (or add one)

## 3. Invite core

- [x] 3.1 Add an `invites` module with code generation (100 random bits, Crockford base32, four dash-separated groups of five), normalization (trim, drop dashes, upper-case, `I`/`L`→`1`, `O`→`0`), and SHA-256 hashing; verify with unit tests for format, distinctness over many codes, and normalization of lower-case, dash-less, and ambiguous-letter input
- [x] 3.2 Add create, list (active or all, with derived status and redeeming username), and revoke-by-ID-suffix storage functions; verify with tests covering each status, ambiguous and unknown suffixes, and revoking used or already revoked invites
- [x] 3.3 Add a read-only `check(code)` and a transactional `consume(tx, code, user_id)` using the single conditional `UPDATE`; verify with tests that expired, revoked, used, unknown, and malformed codes all fail the same way and that `consume` affects exactly one row once

## 4. Operator CLI

- [x] 4.1 Add duration (`m`/`h`/`d`, 1 minute to 365 days) and size (bytes, `KiB`/`MiB`/`GiB`) parsers for clap; verify with unit tests for valid, zero, oversize, and garbage input
- [x] 4.2 Add `invite create [--expires] [--note]`, printing only the code on stdout and the expiry on stderr, migrating the database first; verify with a CLI test that the printed code redeems and the database does not contain it in any form
- [x] 4.3 Add `invite list [--all]` and `invite revoke <id>` with the exit codes from `specs/invites/spec.md`; verify with CLI tests for the default/`--all` listings, redeeming username shown, and revoke success and failure cases
- [x] 4.4 Add `user quota <username> [<size>]` under the existing `user` group; verify with CLI tests for showing usage and quota, setting `1GiB`, lowering below usage, and an unknown username exiting non-zero
- [x] 4.5 Document the `invite` and `user quota` commands in the Commands section of `docs/deployment.md`, including onboarding a new user; verify each documented command runs as written against a scratch database

## 5. Invite-gated registration

- [x] 5.1 Add a required `invite_code` to `RegisterStartRequest` and `RegisterFinishRequest` in `notebook-core::auth::api`, take it in `Registration::start(username, passphrase, invite_code)` in `notebook-core::auth::client`, and add `invalid_invite` to the core auth error codes and `quota_exceeded` to the sync protocol error codes; verify `cargo test -p notebook-core` (including a serde round-trip with the new field) and the wasm32 build check pass
- [x] 5.2 Remove `RegistrationPolicy` and the `registration_closed` error; in `register/start`, run the read-only invite check at the point the policy check ran (before parsing the username or any OPAQUE work) and return `403` `{"error":"invalid_invite"}` on any failure; verify with router tests that a missing, unknown, expired, revoked, or used code gets byte-identical responses, including with a taken or malformed username
- [x] 5.3 Update the `opaque-accounts` tests that used `RegistrationPolicy::Open` to create an invite through the store and pass its code; verify the full `notebook-server` test suite passes with no remaining reference to `RegistrationPolicy` or `registration_closed`
- [x] 5.4 Fill the `register/finish` extension point before `store::create_account` with `consume`, using the already-open `BEGIN IMMEDIATE` transaction and the pre-generated user ID; verify with tests that a valid code creates the account and marks the invite used by that user, and that a `username_taken` or malformed-bundle failure leaves the invite active
- [x] 5.5 Verify concurrent redemption with a test that runs two finishes with the same code and different usernames in parallel and asserts exactly one account exists and the other request got `invalid_invite`
- [x] 5.6 Have `user add` pass an empty invite code to `Registration::start` and keep calling `create_account` directly; verify with a CLI test that `user add` succeeds with no invites present, consumes none, and the user can log in over HTTP
- [x] 5.7 Build the existing registration limiter from `registration_rate_limit_burst` / `registration_rate_limit_per_minute` (adding a per-minute constructor to `KeyedLimiter` if needed) and attach the same instance to `register/finish` as well as `register/start`; verify with router tests that 3 starts plus 3 finishes from one client yield `429` with `Retry-After` on the sixth without touching the database, that a configured burst of 2 limits the third start, and that login from the same client is unaffected

## 6. Vault storage quotas

- [x] 6.1 Add a `quota_bytes` argument to `store::create_account` and pass `default_vault_quota_bytes` from both `register/finish` and `user add`; verify with tests that a vault from each path gets a configured 50 MiB quota and zero usage
- [x] 6.2 Add a `quota` module with `reserve(tx, vault_id, delta)` as one conditional `UPDATE` that always allows non-positive deltas; verify with unit tests at, below, and above quota and for negative deltas over quota
- [x] 6.3 Call `reserve` with the payload size in the HTTP and WebSocket push paths, inside the insert transaction, mapping failure to `507` `{"error":"quota_exceeded"}` over HTTP and a `quota_exceeded` push error frame over WebSocket; verify with tests that an over-quota push stores nothing, is not relayed to other connected clients, and leaves the socket open
- [x] 6.4 Call `reserve` with the net delta in snapshot upload with compaction, and with the negative total in note purge; verify with tests that a shrinking snapshot is accepted at quota, a growing one over quota is rejected, and purge succeeds over quota
- [x] 6.5 Verify accounting stays exact with an integration test that runs pushes, snapshot uploads, compaction, and a purge across two vaults and asserts each `used_bytes` equals the recomputed sum of stored payload lengths

## 7. Integration

- [x] 7.1 Run `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`, and the wasm32 build check, and verify all pass
- [x] 7.2 Verify end to end against a locally served binary: create an invite, register through the HTTP API with it, see it listed as used by that username, fail a second registration with the same code, set a small quota with `user quota`, and observe `507` on a push that exceeds it

## Workflow follow-up

- Code review finding deferred by the user: usage counts payload bytes only, so empty-payload updates (including ones that register a note and its wrapped key) store rows at no quota cost, even over quota.
- Run the security-analyst review (auth, rate limiting, server input handling) before archiving.
- Archive the change after review, then confirm `openspec/specs/invites/spec.md` exists and `openspec/specs/accounts/spec.md` contains the added requirements.
- Update TODO.md item 14 to `[x]`.
