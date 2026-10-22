# invites Specification

## Purpose
Lets the server operator control who can create an account on a publicly hosted server, using single-use, expiring invite codes that are created and managed from the operator command line and redeemed during registration.

## Requirements

### Requirement: Operator creates invite codes
`notebook-server invite create` SHALL generate a new invite code, store it, print the code exactly once on standard output, and exit 0. The command SHALL accept an optional `--expires <duration>` (such as `30m`, `12h`, `7d`) and an optional `--note <text>` recorded with the invite. Without `--expires`, the invite SHALL expire 7 days after creation.

#### Scenario: Default expiry
- **WHEN** an operator runs `notebook-server invite create`
- **THEN** a code is printed, the command exits 0, and `invite list` shows the new invite expiring 7 days after its creation time

#### Scenario: Custom expiry and note
- **WHEN** an operator runs `notebook-server invite create --expires 12h --note "for alice"`
- **THEN** the new invite expires 12 hours after creation and `invite list` shows the note `for alice`

#### Scenario: Invalid duration
- **WHEN** an operator runs `notebook-server invite create --expires soon`
- **THEN** the command exits non-zero with an error naming the `--expires` option and no invite is created

#### Scenario: Expiry out of range
- **WHEN** an operator passes an `--expires` duration that is zero or longer than 365 days
- **THEN** the command exits non-zero and no invite is created

### Requirement: Invite codes are high-entropy and stored only as hashes
Each invite code SHALL contain at least 100 bits from a cryptographically secure random source. The server SHALL store only a one-way hash of the code, never the code itself, and SHALL NOT write the code to logs.

#### Scenario: Database holds no plaintext code
- **WHEN** an invite is created and the database file is searched for the printed code, with and without its separators
- **THEN** the code is not found

#### Scenario: Codes are distinct
- **WHEN** an operator creates many invites
- **THEN** every printed code is different

### Requirement: Invite code format is forgiving to type
Invite codes SHALL use an alphabet without visually ambiguous characters and be printed in dash-separated groups. When a code is redeemed, the server SHALL ignore letter case, dashes, and surrounding whitespace.

#### Scenario: Retyped in lower case without dashes
- **WHEN** a user registers with a valid code typed in lower case with the dashes omitted
- **THEN** the code is accepted

### Requirement: Operator lists invites
`notebook-server invite list` SHALL print one line per invite with its identifier, creation time, expiry time, note, and status (`active`, `used`, `expired`, or `revoked`), and for used invites the username that redeemed it and when. By default only active invites SHALL be listed; `--all` SHALL list every invite. The listing SHALL NOT show codes.

#### Scenario: Default listing shows only active invites
- **WHEN** there is one active, one used, one expired, and one revoked invite and the operator runs `invite list`
- **THEN** only the active invite is printed

#### Scenario: Full listing shows redemption
- **WHEN** an invite was redeemed by user `alice` and the operator runs `invite list --all`
- **THEN** that invite is shown as `used`, with `alice` and the redemption time

### Requirement: Operator revokes invites
`notebook-server invite revoke <id>` SHALL mark an active invite as revoked so it can no longer be redeemed, and exit 0. Revoking an unknown, used, or already revoked invite SHALL exit non-zero with an error saying why and change nothing.

#### Scenario: Revoke an active invite
- **WHEN** an operator revokes an active invite and someone then registers with its code
- **THEN** the registration is rejected as an invalid invite

#### Scenario: Revoke a used invite
- **WHEN** an operator revokes an invite that has already been redeemed
- **THEN** the command exits non-zero, reports that the invite was already used, and the account created with it is unaffected

#### Scenario: Unknown identifier
- **WHEN** an operator runs `invite revoke` with an identifier that matches no invite
- **THEN** the command exits non-zero

### Requirement: Invites are single-use
An invite SHALL be redeemable only while it is unused, unrevoked, and unexpired, and SHALL be consumed by exactly one successful registration. Of any number of concurrent registrations using the same code, at most one SHALL succeed.

#### Scenario: Second use is rejected
- **WHEN** a user has registered with a code and another registration then uses the same code
- **THEN** the second registration is rejected as an invalid invite

#### Scenario: Concurrent redemption
- **WHEN** two registrations with different usernames submit the same valid code at the same time
- **THEN** exactly one account is created and the other registration is rejected as an invalid invite

#### Scenario: Expired code
- **WHEN** a registration uses a code after its expiry time
- **THEN** the registration is rejected as an invalid invite

### Requirement: Invite rejections are indistinguishable
A registration with a code that is unknown, malformed, expired, revoked, or already used SHALL receive the same status and error body, so a caller cannot learn which of these applies or whether a code ever existed.

#### Scenario: Unknown and expired codes look alike
- **WHEN** one registration uses a code that never existed and another uses an expired code
- **THEN** both receive identical status codes and response bodies

### Requirement: Invite redemption is rate limited
Every registration request, start or finish, SHALL draw from the per-address registration limit defined by the `accounts` capability before its invite code is looked up, whether or not the code turns out valid. A request over the limit SHALL get `429` with a `Retry-After` header and SHALL NOT be checked against stored invites.

#### Scenario: Guessing is throttled
- **WHEN** one client sends registration requests with wrong codes faster than the registration limit allows
- **THEN** the excess requests get `429` with a `Retry-After` header

#### Scenario: Finish cannot be used to bypass the limit
- **WHEN** a client completes one registration start and then sends many registration finish requests with different codes
- **THEN** the finish requests beyond the registration limit get `429`

#### Scenario: Independent of login
- **WHEN** a client has exhausted its registration limit and then attempts to log in
- **THEN** the login request is not rejected by the registration limiter
