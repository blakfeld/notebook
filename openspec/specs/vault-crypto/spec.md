# vault-crypto Specification

## Purpose
Defines the end-to-end encryption every Notebook client shares: the vault key hierarchy, per-note keys, the server-stored key bundle, the recovery key, passphrase key derivation, and the versioned authenticated ciphertext envelope.

## Requirements

### Requirement: Vault identity
Every vault SHALL have an identifier that is a ULID, chosen by the client when the vault is created so the key bundle can be bound to it before anything reaches the server. Its text form SHALL be the 26-character Crockford base32 ULID encoding, and parsing that text form SHALL yield the same identifier.

#### Scenario: Vault identifier round-trips through text
- **WHEN** a vault identifier is formatted as text and the text is parsed back
- **THEN** the parsed identifier equals the original

#### Scenario: Invalid vault identifier text is rejected
- **WHEN** text that is not a valid 26-character ULID is parsed as a vault identifier
- **THEN** parsing fails with an error

### Requirement: Random keys
Vault keys and note keys SHALL be 256-bit keys drawn from the platform's cryptographically secure random source, on native targets and in the browser. If the random source fails, key generation SHALL return an error rather than a weak or partial key.

#### Scenario: Generated keys differ
- **WHEN** two vault keys or two note keys are generated
- **THEN** they are different

### Requirement: Authenticated encryption of note data
Note payloads, CRDT updates, and CRDT snapshots SHALL be encrypted with the note's key using XChaCha20-Poly1305 and a fresh random 192-bit nonce per encryption. Decryption SHALL return the original plaintext exactly.

#### Scenario: Round trip
- **WHEN** a payload is encrypted with a note key and decrypted with the same key and context
- **THEN** the result equals the original payload, including an empty payload

#### Scenario: Same plaintext encrypts differently
- **WHEN** the same payload is encrypted twice with the same key and context
- **THEN** the two ciphertexts differ

#### Scenario: Ciphertext hides the plaintext
- **WHEN** a payload containing a distinctive marker string is encrypted
- **THEN** the marker does not appear in the ciphertext

### Requirement: Ciphertext bound to its context
Every ciphertext SHALL be authenticated together with its context: vault ID, note ID, and payload kind. Decrypting under any different context SHALL fail, so a server cannot move a ciphertext to another vault, note, or kind.

#### Scenario: Wrong note
- **WHEN** a ciphertext encrypted for note A is decrypted as belonging to note B with the same key
- **THEN** decryption fails

#### Scenario: Wrong vault
- **WHEN** a ciphertext encrypted for vault V is decrypted as belonging to vault W with the same key
- **THEN** decryption fails

#### Scenario: Wrong payload kind
- **WHEN** a ciphertext encrypted as a CRDT update is decrypted as a snapshot
- **THEN** decryption fails

### Requirement: Tampering and wrong keys are detected
Decryption SHALL fail if any byte of the ciphertext, including its header and nonce, has been changed or if the wrong key is used. The failure SHALL be a single generic error that does not reveal which check failed, and no partial plaintext SHALL be returned.

#### Scenario: Flipped byte
- **WHEN** any single byte of a ciphertext is changed and it is decrypted
- **THEN** decryption fails

#### Scenario: Truncated ciphertext
- **WHEN** a ciphertext is truncated to any shorter length and decrypted
- **THEN** decryption fails without panicking

#### Scenario: Wrong key
- **WHEN** a ciphertext is decrypted with a different note key
- **THEN** decryption fails with the same error as tampering

### Requirement: Versioned ciphertext envelope
Every ciphertext SHALL begin with a format version and an algorithm identifier, followed by the nonce and the authenticated ciphertext. Both header fields SHALL be authenticated. A ciphertext with an unknown version or algorithm SHALL be rejected with an error that names it as unsupported, distinct from an authentication failure.

#### Scenario: Header layout
- **WHEN** a payload is encrypted with the current format
- **THEN** the ciphertext starts with version 1 and algorithm 1 (XChaCha20-Poly1305), and its length is the payload length plus 42 bytes

#### Scenario: Unknown version
- **WHEN** a ciphertext whose version byte is not a known version is decrypted
- **THEN** decryption fails with an unsupported-version error

#### Scenario: Unknown algorithm
- **WHEN** a ciphertext whose algorithm byte is not a known algorithm is decrypted
- **THEN** decryption fails with an unsupported-algorithm error

### Requirement: Per-note keys wrapped by the vault key
Each note key SHALL be stored only wrapped (encrypted) by the vault key, using the envelope bound to the vault ID and note ID. Unwrapping SHALL recover the same note key, and a wrapped note key SHALL NOT unwrap for a different note or vault.

#### Scenario: Wrap and unwrap
- **WHEN** a note key is wrapped by a vault key and unwrapped by the same vault key for the same note
- **THEN** the unwrapped key decrypts data the original key encrypted

#### Scenario: Wrapped key moved to another note
- **WHEN** a note key wrapped for note A is unwrapped for note B
- **THEN** unwrapping fails

#### Scenario: Wrong vault key
- **WHEN** a wrapped note key is unwrapped with a different vault key
- **THEN** unwrapping fails

### Requirement: Key bundle
The key bundle SHALL hold the vault ID and the vault key wrapped separately under an auth secret and under the recovery key, and SHALL serialize to bytes the server can store without being able to unwrap. Either secret alone SHALL recover the vault key.

#### Scenario: Unlock with the auth secret
- **WHEN** a bundle is created for a vault key and unlocked with the same auth secret
- **THEN** the recovered vault key unwraps note keys the original wrapped

#### Scenario: Unlock with the recovery key
- **WHEN** a bundle is unlocked with the recovery key it was created with
- **THEN** the recovered vault key unwraps note keys the original wrapped

#### Scenario: Wrong secret
- **WHEN** a bundle is unlocked with a different auth secret or a different recovery key
- **THEN** unlocking fails

#### Scenario: Serialization round trip
- **WHEN** a bundle is serialized, parsed back, and unlocked
- **THEN** unlocking succeeds and the parsed bundle reports the same vault ID

#### Scenario: Malformed bundle bytes
- **WHEN** truncated or garbage bytes are parsed as a key bundle
- **THEN** parsing fails with an error without panicking

### Requirement: Auth secret input
An auth secret SHALL be accepted from the auth layer (such as the OPAQUE export key) or from passphrase derivation as at least 32 bytes of high-entropy input. Shorter input SHALL be rejected. The key that wraps the vault key SHALL be derived from the secret and the vault ID, so the same secret yields unrelated wrapping keys for different vaults.

#### Scenario: Short secret rejected
- **WHEN** an auth secret of fewer than 32 bytes is supplied
- **THEN** it is rejected with an error

#### Scenario: Bundle bound to its vault
- **WHEN** the wrapped vault key from a bundle for vault V is placed in a bundle claiming vault W and unlocked with the same secret
- **THEN** unlocking fails

### Requirement: Rewrapping on passphrase change
Replacing the auth secret SHALL rewrap only the vault key. The recovery entry, the vault key, note keys, and all note ciphertexts SHALL be unchanged, and the old auth secret SHALL no longer unlock the bundle.

#### Scenario: Passphrase change
- **WHEN** a bundle's auth entry is rewrapped under a new auth secret
- **THEN** the new secret and the recovery key unlock the same vault key, and the old secret fails

### Requirement: Recovery key
At vault creation the client SHALL generate a recovery key with at least 128 bits of randomness, shown as uppercase Crockford base32 in hyphen-separated groups of four, with an embedded checksum. It SHALL independently unwrap the vault key from the bundle.

#### Scenario: Display format
- **WHEN** a recovery key is generated and displayed
- **THEN** it is 8 groups of 4 characters from the Crockford base32 alphabet separated by hyphens

#### Scenario: Two recovery keys differ
- **WHEN** two recovery keys are generated
- **THEN** their displayed forms differ

### Requirement: Forgiving recovery key entry
Parsing a recovery key SHALL ignore case, spaces, and hyphens, and SHALL read `I` and `L` as `1` and `O` as `0`. A key whose checksum does not match SHALL be rejected as invalid before any decryption is attempted, with an error distinct from a wrong-but-valid key.

#### Scenario: Sloppy but correct entry
- **WHEN** a displayed recovery key is re-entered in lowercase, with spaces instead of hyphens, and with `O` for `0`
- **THEN** it parses to the same recovery key

#### Scenario: Typo detected
- **WHEN** one character of a displayed recovery key is changed and the result is parsed
- **THEN** parsing fails with an invalid-recovery-key error

#### Scenario: Wrong length
- **WHEN** text with too few or too many characters is parsed as a recovery key
- **THEN** parsing fails with an invalid-recovery-key error

### Requirement: Passphrase key derivation
Deriving a key directly from a passphrase SHALL use Argon2id with a random salt of at least 16 bytes and a numbered parameter set. Version 1 SHALL be 64 MiB memory, 3 iterations, parallelism 1. Unknown versions SHALL be rejected rather than falling back to weaker settings.

#### Scenario: Deterministic derivation
- **WHEN** the same passphrase, salt, and parameter version are derived twice
- **THEN** both derivations produce the same auth secret

#### Scenario: Salt and passphrase matter
- **WHEN** a passphrase is derived with two different salts, or two passphrases with the same salt
- **THEN** the results differ

#### Scenario: Unknown parameter version
- **WHEN** a derivation is requested with a parameter version that is not defined
- **THEN** it fails with an error

### Requirement: Key material is zeroized
Vault keys, note keys, recovery keys, auth secrets, and derived wrapping keys SHALL be overwritten in memory when dropped. Their debug output SHALL NOT include key bytes, and decrypted plaintext SHALL be returned in a container that zeroizes on drop.

#### Scenario: Debug output is redacted
- **WHEN** any key type or auth secret is formatted with debug formatting
- **THEN** the output contains no key bytes

### Requirement: Same behavior on every client
The vault cryptography SHALL build for native targets and for `wasm32-unknown-unknown` with the browser's secure random source, and ciphertexts produced on one target SHALL decrypt on another.

#### Scenario: Browser build
- **WHEN** the core crate is built for `wasm32-unknown-unknown`
- **THEN** the build succeeds with the cryptography included

#### Scenario: Fixed test vector
- **WHEN** a fixed key, nonce, context, and payload are encrypted through the envelope
- **THEN** the output matches a recorded byte string, so any target producing different bytes fails the test
