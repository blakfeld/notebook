# Tasks

## 1. Dependencies and module skeleton

- [x] 1.1 Add `chacha20poly1305`, `argon2`, `hkdf`, `sha2`, `zeroize`, and `getrandom` (with `wasm_js` only for `wasm32-unknown-unknown`) to the workspace and `notebook-core`; add the dev-profile `opt-level = 3` overrides for `argon2` and its hash; create the empty `crypto/{mod,envelope,keys,secret,recovery,bundle,kdf}.rs` modules re-exported from `lib.rs`; verify `cargo build -p notebook-core` and the wasm32 build both succeed

## 2. Envelope and identifiers

- [x] 2.1 Implement `CryptoError`, `VaultId` (ULID newtype with `from_parts`, `Display`, `FromStr`), and the internal random-fill helper; verify unit tests for vault ID round-trip and invalid-text rejection
- [x] 2.2 Implement the envelope (`seal`, `seal_with_nonce`, `open`, AAD encoding, header checks); verify unit tests for round trip, empty payload, header layout and 42-byte overhead, unknown version and algorithm errors, every single-byte flip, every truncation, and a fixed test vector

## 3. Vault and note keys

- [x] 3.1 Implement `SymmetricKey`, `VaultKey`, `NoteKey`, and `PayloadKind` with zeroize-on-drop and redacted `Debug`; implement `NoteKey::encrypt`/`decrypt` returning `Zeroizing` plaintext; verify unit tests for the random-keys, authenticated-encryption, context-binding, tampering/wrong-key, and debug-redaction scenarios
- [x] 3.2 Implement `VaultKey::wrap_note_key`/`unwrap_note_key`; verify unit tests for wrap/unwrap, wrapped key moved to another note, and wrong vault key

## 4. Secrets, recovery key, and passphrase derivation

- [x] 4.1 Implement `AuthSecret` (minimum 32 bytes) and HKDF wrapping-key derivation for auth and recovery; verify unit tests for short-secret rejection and that different vault IDs give different wrapping keys
- [x] 4.2 Implement `RecoveryKey` generation, checksum, `to_display`, and forgiving `parse`; verify unit tests for display format, uniqueness, sloppy-but-correct entry, every single-character typo, and wrong length
- [x] 4.3 Implement `Salt`, `KdfVersion`, and `derive_auth_secret` with Argon2id V1 parameters; verify unit tests for determinism, salt and passphrase sensitivity, and unknown-version rejection

## 5. Key bundle

- [x] 5.1 Implement `KeyBundle::create`, `unlock_with_auth`, `unlock_with_recovery`, `rewrap_auth`, `vault_id`, `to_bytes`, and `from_bytes`; verify unit tests for every key-bundle, bundle-bound-to-vault, and passphrase-change scenario, including malformed and truncated bytes
- [x] 5.2 Add rustdoc on the public `crypto` API covering the key hierarchy, what each secret unlocks, and the envelope layout; verify `cargo doc -p notebook-core --no-deps` builds without warnings

## 6. Integration checks

- [x] 6.1 Run `scripts/check.sh` and verify it exits 0 (fmt, clippy with `-D warnings`, workspace tests, wasm32 core build)
- [x] 6.2 Run `openspec validate vault-encryption --strict` and verify it passes

## Workflow follow-up

- Review the change, then archive it with `/opsx:archive` so the `vault-crypto` spec lands in `openspec/specs/`.
- Update `TODO.md`: mark `vault-encryption` as `[x]` once archived.
