# Spec Delta

## ADDED Requirements

### Requirement: Recovery verifier
A recovery key SHALL deterministically yield a 32-byte recovery verifier that a server can store to check possession of the recovery key. The verifier SHALL be derived with a domain separate from the recovery wrapping key, so knowing it gives no help in unwrapping the vault key, and different recovery keys SHALL yield different verifiers.

#### Scenario: Deterministic verifier
- **WHEN** the verifier is derived twice from the same recovery key, once from the generated key and once from its re-entered display form
- **THEN** both verifiers are identical

#### Scenario: Different keys
- **WHEN** verifiers are derived from two different recovery keys
- **THEN** they differ

#### Scenario: Verifier does not unlock the bundle
- **WHEN** the verifier bytes are used as an auth secret to unlock a bundle created with that recovery key
- **THEN** unlocking fails

#### Scenario: Fixed test vector
- **WHEN** the verifier is derived from a fixed recovery key
- **THEN** it matches a recorded byte string, so every target derives the same verifier
