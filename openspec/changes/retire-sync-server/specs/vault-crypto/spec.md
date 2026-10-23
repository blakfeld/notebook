# Spec Delta

## MODIFIED Requirements

### Requirement: Vault identity
Every vault SHALL have an identifier that is a ULID, chosen by the client when the vault is created so the recovery entry and every ciphertext can be bound to it. Its text form SHALL be the 26-character Crockford base32 ULID encoding, and parsing that text form SHALL yield the same identifier.

#### Scenario: Vault identifier round-trips through text
- **WHEN** a vault identifier is formatted as text and the text is parsed back
- **THEN** the parsed identifier equals the original

#### Scenario: Invalid vault identifier text is rejected
- **WHEN** text that is not a valid 26-character ULID is parsed as a vault identifier
- **THEN** parsing fails with an error

### Requirement: Ciphertext bound to its context
Every ciphertext SHALL be authenticated together with its context: vault ID, note ID, and payload kind. Decrypting under any different context SHALL fail, so ciphertext copied out of a database or backup cannot be moved to another vault, note, or kind.

#### Scenario: Wrong note
- **WHEN** a ciphertext encrypted for note A is decrypted as belonging to note B with the same key
- **THEN** decryption fails

#### Scenario: Wrong vault
- **WHEN** a ciphertext encrypted for vault V is decrypted as belonging to vault W with the same key
- **THEN** decryption fails

#### Scenario: Wrong payload kind
- **WHEN** a ciphertext encrypted as a CRDT update is decrypted as a snapshot
- **THEN** decryption fails

## REMOVED Requirements

### Requirement: Key bundle
**Reason**: The key bundle existed for server storage alongside OPAQUE. The local vault and backups use the recovery entry instead.
**Migration**: Upgraded installs get a recovery entry from `local-only-desktop`.

### Requirement: Auth secret input
**Reason**: There is no auth layer. Passphrase derivation remains only for the legacy-install upgrade.
**Migration**: None.

### Requirement: Rewrapping on passphrase change
**Reason**: There is no passphrase.
**Migration**: None.

### Requirement: Recovery verifier
**Reason**: It let a server check possession of the recovery key, and there is no server.
**Migration**: None.
