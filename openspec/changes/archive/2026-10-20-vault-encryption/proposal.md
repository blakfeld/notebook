# Proposal

## Why

Notes must be end-to-end encrypted: the publicly hosted server stores only ciphertext and can never read note content, titles, or tags. Every client (native and browser) needs one shared, audited key hierarchy and encryption scheme in `notebook-core`. A second implementation would eventually diverge and leave notes unreadable on some client.

## What Changes

- Define the vault key hierarchy in `notebook-core`:
  - A random **vault key** per account.
  - Random **per-note keys**, each wrapped (encrypted) by the vault key.
  - A **key bundle** (the wrapped vault key) designed to be stored on the server and unwrapped by a client-held secret. The secret comes from the auth layer, e.g. the OPAQUE export key, or from the recovery key.
- Authenticated encryption with XChaCha20-Poly1305 for note payloads, CRDT updates, and snapshots, with associated data binding each ciphertext to its vault ID, note ID, and payload kind, so ciphertexts can't be swapped between contexts.
- A **recovery key**: a high-entropy, human-transcribable code generated at vault creation that can independently unwrap the vault key.
- Argon2id passphrase-based key derivation for any path that derives keys from a passphrase directly, with versioned parameters.
- Versioned ciphertext envelope format (algorithm and version header) so primitives can change later.
- Zeroize key material in memory when it's no longer needed.

## Capabilities

### New Capabilities
- `vault-crypto`: Vault key hierarchy, per-note keys, key-bundle wrapping/unwrapping, recovery key, authenticated encryption of note payloads, and the versioned ciphertext envelope.

### Modified Capabilities
<!-- None. -->

## Impact

- `notebook-core`: new `crypto` module. Must compile to `wasm32` (pure-Rust crates such as `chacha20poly1305`, `argon2`, `zeroize`, with `getrandom` using its JS backend on wasm).
- No server or UI changes yet. Accounts and sync consume this.
- Security-critical: the design should record the threat model (an honest-but-curious or compromised server, a stolen database) and what metadata stays visible (note IDs, sizes, timing).
