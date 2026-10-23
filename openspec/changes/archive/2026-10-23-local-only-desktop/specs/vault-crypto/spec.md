# Spec Delta

## ADDED Requirements

### Requirement: Recovery entry
A recovery entry SHALL hold the vault ID and the vault key wrapped under the recovery key, bound to that vault ID, with no auth secret. It SHALL serialize to versioned bytes that can be stored beside the vault and inside backups. Only the matching recovery key SHALL unwrap it.

#### Scenario: Unlock with the recovery key
- **WHEN** a recovery entry is created for a vault key and opened with the same recovery key
- **THEN** the recovered vault key unwraps note keys the original wrapped

#### Scenario: Wrong recovery key
- **WHEN** a recovery entry is opened with a different recovery key
- **THEN** opening fails with the generic decryption error

#### Scenario: Bound to its vault
- **WHEN** the wrapped key from an entry for vault V is placed in an entry claiming vault W and opened with the right recovery key
- **THEN** opening fails

#### Scenario: Serialization round trip
- **WHEN** an entry is serialized, parsed back, and opened
- **THEN** opening succeeds and the parsed entry reports the same vault ID

#### Scenario: Malformed entry bytes
- **WHEN** truncated or garbage bytes, or bytes with an unknown version, are parsed as a recovery entry
- **THEN** parsing fails with an error without panicking

#### Scenario: Fixed test vector
- **WHEN** a recovery entry is created from a fixed vault key, recovery key, vault ID, and nonce
- **THEN** its bytes match a recorded byte string, so every build reads entries written by earlier ones
