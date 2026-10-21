# Spec Delta

## Purpose

Defines Notebook accounts: OPAQUE-based registration and login that never reveal the passphrase to the server, device sessions, the server-stored encrypted key bundle, passphrase change, recovery-key account recovery, and the operator command for creating accounts.

## ADDED Requirements

### Requirement: Usernames
A username SHALL be 3 to 32 characters of ASCII letters, digits, `.`, `_`, and `-`, starting with a letter or digit. Usernames SHALL be case-insensitive: the client and server SHALL both normalize to lowercase before any OPAQUE operation, lookup, or storage. A username that violates these rules SHALL be rejected with an `invalid_username` error.

#### Scenario: Case-insensitive login
- **WHEN** an account is created as `Alice` and the user later logs in as `ALICE` with the correct passphrase
- **THEN** login succeeds for the same account

#### Scenario: Invalid characters
- **WHEN** a client registers with the username `al ice` or `ålice`
- **THEN** the request is rejected with `invalid_username` and nothing is created

#### Scenario: Length limits
- **WHEN** a username of 2 characters or of 33 characters is submitted
- **THEN** it is rejected with `invalid_username`

### Requirement: Passphrase never reaches the server
The client SHALL authenticate using OPAQUE, so that no request carries the passphrase or a value from which the server could check a passphrase guess without the OPAQUE server key. The client SHALL reject passphrases shorter than 12 characters before contacting the server; the server cannot check length and does not try.

#### Scenario: Passphrase absent from traffic
- **WHEN** a user registers and logs in with a passphrase containing a distinctive marker string
- **THEN** no request body or header sent to the server contains the marker

#### Scenario: Short passphrase
- **WHEN** a user tries to register, recover, or change to an 11-character passphrase
- **THEN** the client refuses before sending any request

### Requirement: Registration creates an account and vault
Completing registration SHALL atomically create the user, one vault whose ID is the client-chosen vault ID in the key bundle, the key bundle, and the recovery verifier, and SHALL return a new device session. The server SHALL reject a bundle that does not parse or whose vault ID is already in use, creating nothing.

#### Scenario: Successful registration
- **WHEN** a client completes registration with a new username, a valid key bundle, and a recovery verifier
- **THEN** the account, vault, and bundle exist, the response carries an access token and refresh token, and logging in with the same passphrase returns the same bundle bytes

#### Scenario: Malformed bundle
- **WHEN** a client completes registration with key bundle bytes that do not parse
- **THEN** the request is rejected with `invalid_request` and no user, vault, or bundle is created

### Requirement: Recovery key shown once at registration
The client SHALL generate the vault key and recovery key locally during registration and SHALL return the recovery key to its caller exactly once for display. The recovery key SHALL NOT be sent to the server.

#### Scenario: Recovery key stays on the client
- **WHEN** a client registers and the generated recovery key is known to the test
- **THEN** no request sent to the server contains the recovery key in any encoding

### Requirement: Taken usernames at registration
Registration SHALL fail with `username_taken` (HTTP `409`) when the normalized username already exists, including when another registration for it completed between this registration's start and finish. A failed registration SHALL create nothing.

#### Scenario: Duplicate username
- **WHEN** a client registers `bob` and an account `bob` already exists
- **THEN** the registration fails with `username_taken` and the existing account is unchanged

#### Scenario: Race between two registrations
- **WHEN** two registrations for `carol` both start and then both try to finish
- **THEN** exactly one account `carol` exists and the other registration fails with `username_taken`

### Requirement: Login returns a session and key bundle
A successful OPAQUE login SHALL return a new device session (access token, its lifetime, refresh token, session ID), the account's username and admin flag, and the vault's key bundle. The client SHALL unlock the bundle with the OPAQUE export key and SHALL report a failed unlock as an error rather than proceeding.

#### Scenario: Login unlocks the vault
- **WHEN** a user logs in with the correct passphrase
- **THEN** the client recovers the vault key, and it unwraps a note key wrapped at registration

#### Scenario: Wrong passphrase
- **WHEN** a user logs in with an incorrect passphrase
- **THEN** login fails with `invalid_credentials` (HTTP `401`), no session is created, and no key bundle is returned

### Requirement: Login does not reveal whether an account exists
For a username with no account, the login start response SHALL have the same shape and size as for an existing account, and the login SHALL fail at the finish step with the same `invalid_credentials` error as a wrong passphrase.

#### Scenario: Unknown username
- **WHEN** a client logs in as a username that has no account
- **THEN** the start step succeeds with a response of the same size as for an existing account, and the finish step fails with `invalid_credentials`

#### Scenario: Stable fake response
- **WHEN** login is started twice for the same unknown username with the same client request
- **THEN** the server-provided parts derived from the username are identical both times, as they would be for a real account

### Requirement: Pending handshakes expire
Server state held between the start and finish steps of login, passphrase change, and recovery SHALL be single-use and SHALL expire 60 seconds after the start step. Finishing with an expired, unknown, or already-used handshake ID SHALL fail with `invalid_credentials`.

#### Scenario: Late finish
- **WHEN** a client sends the login finish step more than 60 seconds after the start step
- **THEN** it fails with `invalid_credentials`

#### Scenario: Replayed finish
- **WHEN** a client repeats a login finish request that already succeeded
- **THEN** the repeat fails with `invalid_credentials` and no second session is created

### Requirement: Current key bundle can be fetched
An authenticated client SHALL be able to fetch its vault's current key bundle bytes, so a signed-in device can detect that the bundle changed (for example after a passphrase change on another device) by comparing it with the bundle it holds.

#### Scenario: Bundle after a passphrase change elsewhere
- **WHEN** device A changes the passphrase while device B is signed in, and B then refreshes its session and fetches the key bundle
- **THEN** B receives the new bundle bytes, which differ from the bundle B received at login

#### Scenario: Unchanged bundle
- **WHEN** a signed-in client fetches the key bundle twice with no passphrase change in between
- **THEN** both responses carry identical bytes, equal to those returned at login

### Requirement: Access tokens authenticate requests
Authenticated endpoints SHALL require `Authorization: Bearer <access token>`. An access token SHALL be valid for 15 minutes from issue and only while its session exists. A missing, malformed, expired, or revoked token SHALL get `401` with `invalid_token`, and the request SHALL identify exactly one user, vault, and session.

#### Scenario: Valid token
- **WHEN** a client calls an authenticated endpoint with a current access token
- **THEN** the request is handled on behalf of that token's user and vault

#### Scenario: Expired token
- **WHEN** a client uses an access token more than 15 minutes after it was issued
- **THEN** the response is `401` with `invalid_token`

#### Scenario: Missing token
- **WHEN** a client calls an authenticated endpoint without an `Authorization` header
- **THEN** the response is `401` with `invalid_token`

### Requirement: Refresh tokens rotate
Exchanging a refresh token SHALL return a new access token and a new refresh token for the same session, and the presented refresh token and the session's previous access token SHALL stop working. A refresh token SHALL expire 30 days after it was issued.

#### Scenario: Refresh
- **WHEN** a client exchanges its current refresh token
- **THEN** it receives a new token pair, the new access token works, and the old access token gets `401`

#### Scenario: Idle session expires
- **WHEN** a refresh token is presented more than 30 days after it was issued
- **THEN** the exchange fails with `session_ended` and the session no longer exists

### Requirement: Refresh distinguishes an ended session from transient failure
A refresh that fails because the session is gone (revoked, logged out, expired, ended by reuse detection, or never existed) SHALL return `401` with `session_ended`, and the client SHALL report it as a distinct "session ended, log in again" outcome. Any other failure (network error, `429`, `5xx`, timeout) SHALL be reported as transient and SHALL NOT discard the stored refresh token.

#### Scenario: Revoked device refreshes
- **WHEN** a session is revoked from another device and the revoked device then tries to refresh
- **THEN** the server responds `401` with `session_ended` and the client reports the session-ended outcome

#### Scenario: Server unavailable during refresh
- **WHEN** a refresh attempt gets a `503` or no response
- **THEN** the client reports a transient failure and the refresh token remains usable for a later attempt

### Requirement: Refresh token reuse revokes the session
Presenting a refresh token that was already exchanged SHALL revoke the whole session, so a stolen refresh token used after the legitimate client (or vice versa) ends the session for both.

#### Scenario: Reused refresh token
- **WHEN** a refresh token is exchanged, and the same token is presented again
- **THEN** the second exchange fails with `session_ended`, and the tokens issued by the first exchange also stop working

### Requirement: Device sessions are listed and revocable
An authenticated user SHALL be able to list their sessions with ID, device name, creation time, last-used time, and which one is current, and to revoke any of them. Logout SHALL revoke the current session. Revoking a session SHALL invalidate its access and refresh tokens immediately. A user SHALL NOT see or revoke another user's sessions.

#### Scenario: Revoke another device
- **WHEN** a user logged in on two devices revokes the other device's session
- **THEN** that device's access token gets `401` and its refresh token fails, while the current device keeps working

#### Scenario: Logout
- **WHEN** a client logs out
- **THEN** its access and refresh tokens stop working

#### Scenario: Other users' sessions
- **WHEN** a user tries to revoke a session ID that belongs to another user
- **THEN** the response is `404` and that session keeps working

### Requirement: Device names
Login, registration, and recovery SHALL accept a device name of up to 64 characters, shown in the session list. A longer name SHALL be rejected with `invalid_request`; an absent name SHALL be stored as empty.

#### Scenario: Named device
- **WHEN** a client logs in with device name `Work laptop` and lists sessions
- **THEN** the current session is listed as `Work laptop`

### Requirement: Browser sessions are short-lived
Login, registration, and recovery SHALL accept a session kind, `native` (the default) or `browser`. A browser session's refresh token SHALL expire 12 hours after it was issued instead of 30 days, so sessions abandoned by closed tabs end on their own. The kind SHALL be shown in the session list.

#### Scenario: Abandoned browser session
- **WHEN** a browser session's refresh token is presented more than 12 hours after it was issued
- **THEN** the exchange fails with `session_ended`

#### Scenario: Native session unaffected
- **WHEN** a native session's refresh token is presented 13 hours after it was issued
- **THEN** the exchange succeeds

### Requirement: Tokens travel only in headers and bodies
HTTP endpoints SHALL accept access tokens only in the `Authorization: Bearer` header and refresh tokens only in the refresh request body. The server SHALL NOT accept tokens in URLs or set or read cookies for authentication, and logout and refresh SHALL work for any client that holds its tokens in memory.

#### Scenario: No cookies
- **WHEN** a client registers, logs in, refreshes, and logs out
- **THEN** no response carries a `Set-Cookie` header

### Requirement: Passphrase change
An authenticated user SHALL be able to change their passphrase only by also completing an OPAQUE login with the current passphrase in the same operation. On success the server SHALL replace the OPAQUE record and key bundle atomically and revoke every other session of the user; the current session SHALL continue.

#### Scenario: Change passphrase
- **WHEN** a user changes their passphrase with the correct current passphrase
- **THEN** logging in with the new passphrase unlocks the same vault key, the old passphrase fails with `invalid_credentials`, and the recovery key still unlocks the vault

#### Scenario: Wrong current passphrase
- **WHEN** a user attempts a passphrase change with an incorrect current passphrase
- **THEN** it fails with `invalid_credentials` and the OPAQUE record and bundle are unchanged

#### Scenario: Other devices signed out
- **WHEN** a user with two sessions changes their passphrase from one of them
- **THEN** the other session's tokens stop working and the changing session's tokens keep working

### Requirement: Bundle updates preserve the recovery entry
When a passphrase change or recovery uploads a replacement key bundle, the server SHALL reject it with `invalid_request` unless it parses, names the same vault ID, and carries a recovery entry byte-identical to the stored bundle's, so an attacker holding a session cannot destroy the user's ability to recover.

#### Scenario: Altered recovery entry
- **WHEN** a passphrase change uploads a bundle whose recovery entry differs from the stored one
- **THEN** the change is rejected with `invalid_request` and the stored record and bundle are unchanged

#### Scenario: Different vault
- **WHEN** a replacement bundle names a different vault ID
- **THEN** it is rejected with `invalid_request`

### Requirement: Account recovery with the recovery key
A user who has lost their passphrase SHALL be able to recover by presenting their username and the recovery verifier derived from their recovery key, receiving the key bundle, and completing OPAQUE re-registration with a new passphrase and a re-wrapped bundle. Success SHALL revoke every existing session and return a new one.

#### Scenario: Successful recovery
- **WHEN** a user recovers with the correct recovery key and a new passphrase
- **THEN** the new passphrase logs in and unlocks the same vault key, the old passphrase fails, every pre-existing session's tokens stop working, and the response carries a new session

#### Scenario: Recovery key unchanged
- **WHEN** a user recovers and later recovers again with the same recovery key
- **THEN** the second recovery also succeeds

### Requirement: Recovery does not reveal whether an account exists
A recovery attempt with a wrong recovery verifier and one for a username with no account SHALL fail with the same `invalid_credentials` error, and neither SHALL return a key bundle or change any data.

#### Scenario: Wrong recovery key
- **WHEN** a user starts recovery with a well-formed recovery key that is not theirs
- **THEN** it fails with `invalid_credentials`, no bundle is returned, and the account's passphrase still works

#### Scenario: Unknown username
- **WHEN** recovery is started for a username that has no account
- **THEN** it fails with the same `invalid_credentials` error

### Requirement: Login attempts are rate limited
Login start requests SHALL be limited per client address (burst 10, refilling one every 6 seconds) and per normalized username (burst 10, refilling one per minute), applied identically whether or not the username exists. Over-limit requests SHALL get `429` with `Retry-After` and SHALL do no OPAQUE work.

#### Scenario: Guessing one account
- **WHEN** clients from many addresses send more than 10 login attempts for one username within a minute
- **THEN** the excess attempts get `429`

#### Scenario: Unknown usernames limited the same
- **WHEN** a client sends more than 10 login attempts within a minute for a username with no account
- **THEN** the excess attempts get `429`, just as for an existing account

#### Scenario: Other users unaffected
- **WHEN** one username is rate limited and a different user logs in from another address
- **THEN** that login proceeds normally

### Requirement: Recovery and registration are rate limited
Recovery start requests SHALL be limited per client address (burst 5, refilling one every 10 minutes) and registration start requests per client address (burst 5, refilling one per minute). Over-limit requests SHALL get `429` with `Retry-After`.

#### Scenario: Recovery burst
- **WHEN** one client sends 6 recovery attempts in quick succession
- **THEN** the sixth gets `429`

### Requirement: Credentials stored only in protected form
The server SHALL store access tokens, refresh tokens, and recovery verifiers only as SHA-256 hashes, and SHALL compare them in constant time. The database SHALL NOT contain any passphrase, export key, vault key, or recovery key.

#### Scenario: Database contents
- **WHEN** a user registers and logs in, and the database file is searched for the issued access token, refresh token, recovery verifier, and passphrase
- **THEN** none of them appears in the file

### Requirement: Auth errors are uniform JSON
Every error returned by an `/auth` endpoint handler (as opposed to the shared rate-limit, size, and timeout layers) SHALL be a JSON object with a single `error` field holding a snake_case code (`invalid_request`, `invalid_username`, `invalid_credentials`, `invalid_token`, `session_ended`, `username_taken`), and SHALL NOT include internal details.

#### Scenario: Error body
- **WHEN** a login finish fails because of a wrong passphrase
- **THEN** the body is exactly `{"error":"invalid_credentials"}`

### Requirement: OPAQUE server key
The server SHALL keep its OPAQUE key material in a key file at the configured path, outside the database, readable only by its owner. On start, if the file is absent and no accounts exist, the server SHALL create it; it SHALL record the key's public fingerprint in the database.

#### Scenario: First start
- **WHEN** the server starts with no key file and an empty database
- **THEN** it creates the key file with owner-only permissions and records its fingerprint

#### Scenario: Key survives restart
- **WHEN** the server is restarted with the same key file
- **THEN** existing users can still log in

### Requirement: Missing or mismatched server key is refused
The server SHALL refuse to start, exiting non-zero with an error naming the key file setting, when accounts exist but the key file is absent, or when the key file's fingerprint differs from the one recorded in the database. It SHALL NOT generate a replacement key in either case.

#### Scenario: Key file lost
- **WHEN** the server starts against a database with accounts and the key file is missing
- **THEN** it exits non-zero and no new key file is created

#### Scenario: Wrong key file
- **WHEN** the server starts with a key file from a different deployment
- **THEN** it exits non-zero with an error saying the key does not match the database

### Requirement: Operator creates accounts
`notebook-server user add <username> [--admin]` SHALL prompt for a passphrase twice without echo, run the client side of registration locally, create the account with its vault and key bundle directly in the database, and print the recovery key once. The user's first client sign-in is an ordinary login. It SHALL exit non-zero without creating anything if the username is invalid or taken, the passphrases differ, or the passphrase is too short.

#### Scenario: Add a user
- **WHEN** an operator runs `notebook-server user add alice` and enters the same valid passphrase twice
- **THEN** the command prints a recovery key, exits 0, and alice can log in over HTTP with that passphrase

#### Scenario: First sign-in after user add
- **WHEN** a user created with `user add` signs in from a client for the first time
- **THEN** the login returns the existing key bundle, it unlocks with the passphrase, and the client creates no new vault or recovery key

#### Scenario: Admin flag
- **WHEN** an operator runs `notebook-server user add root --admin`
- **THEN** the created account's admin flag is set and login reports it

#### Scenario: Taken username
- **WHEN** an operator runs `user add` for an existing username
- **THEN** the command exits non-zero and the existing account is unchanged

### Requirement: Expired sessions are removed
The server SHALL delete sessions whose refresh token has expired, at least once per hour while running, so expired sessions do not accumulate.

#### Scenario: Cleanup
- **WHEN** a session's refresh token expired more than an hour ago and the server has been running throughout
- **THEN** the session no longer appears in the database

### Requirement: Client auth runs in the browser
The client side of registration, login, passphrase change, and recovery SHALL build for `wasm32-unknown-unknown` and produce messages the native server accepts.

#### Scenario: Browser build
- **WHEN** the core crate is built for `wasm32-unknown-unknown`
- **THEN** the build succeeds with the client auth flows included
