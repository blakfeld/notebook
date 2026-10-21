# Design

## Context

See proposal.md (Why). `notebook-core` holds the `note` and `tags` modules, forbids `unsafe`, and CI builds it for `wasm32-unknown-unknown`. The `note` module takes time and randomness from the caller to stay deterministic. Accounts (`opaque-accounts`) will produce the auth secret (the OPAQUE export key) and store the key bundle; `crdt-note-documents` and sync will encrypt updates and snapshots. This change gives them one implementation to call. Requirements are in `specs/vault-crypto/spec.md`.

## Threat model

- **Honest-but-curious or compromised server, and a stolen server database.** The attacker sees every key bundle, wrapped note key, and ciphertext, and can drop, replay, reorder, or swap them. They must not learn note content, titles, or tags, and must not be able to make a client accept a ciphertext in a context it wasn't written for (another vault, note, or payload kind).
- **Out of scope here:** a compromised client device or browser, malicious JavaScript served by a compromised server to the browser client (a known limit of any web E2EE client), and side channels beyond what the RustCrypto crates already defend against.
- **Visible metadata:** vault IDs, note IDs, the number of notes, ciphertext sizes (plaintext length plus a fixed 42 bytes), and the timing of writes. Padding is not done in this change.
- **Not detected by this layer:** rollback or withholding (serving an older valid ciphertext, or omitting one). Ordering belongs to the sync protocol; the CRDT tolerates replayed updates.

## Goals / Non-Goals

**Goals:**
- A small, misuse-resistant API: callers never handle nonces, raw key bytes, or AAD encoding.
- One envelope format for every ciphertext, with fixed test vectors so all targets agree.

**Non-Goals:**
- OPAQUE itself and where bundles are stored (`opaque-accounts`).
- Key rotation for the vault key or note keys. The envelope version and per-note keys leave room for it later.
- Padding or hiding sizes and timing.
- Serde derives. Bundles and envelopes are opaque byte strings with their own versioned layout.

## Decisions

### Module layout

```
crates/notebook-core/src/crypto/
  mod.rs       CryptoError, VaultId, random helper, re-exports
  envelope.rs  header, AAD encoding, seal/open over a 32-byte key
  keys.rs      VaultKey, NoteKey, PayloadKind, note-key wrapping
  secret.rs    AuthSecret, HKDF wrapping-key derivation
  recovery.rs  RecoveryKey generation, display, parsing
  bundle.rs    KeyBundle create/unlock/rewrap/serialize
  kdf.rs       Argon2id passphrase derivation, KdfVersion
```

### Primitives

- `chacha20poly1305` 0.11 (`XChaCha20Poly1305`), `argon2` 0.6, `hkdf` 0.13 with `sha2` 0.11, `zeroize` 1.9, `getrandom` 0.4. All are pure Rust and RustCrypto-maintained, all build for wasm32.
- XChaCha20-Poly1305 over ChaCha20-Poly1305 or AES-GCM: the 192-bit nonce makes random nonces safe for any realistic number of messages per key, so there is no nonce counter to persist or sync across devices. AES-GCM without hardware support (wasm) is slow and has timing hazards in software.

### Randomness is drawn inside the crypto module

Unlike `note`, the crypto module calls `getrandom` itself for keys, nonces, salts, and recovery keys. Letting callers supply nonces would make nonce reuse (which breaks Poly1305 authenticity) one mistake away. On `wasm32-unknown-unknown`, `getrandom` 0.4's `wasm_js` feature (enabled only for that target in `notebook-core/Cargo.toml`) uses `crypto.getRandomValues`. RNG failure surfaces as `CryptoError::Random`, never a panic. Tests that need fixed bytes (the test vector) call an internal `seal_with_nonce` that is `pub(crate)`.

**Alternative considered:** caller-supplied randomness for determinism. Rejected: determinism matters less here than removing the nonce-reuse footgun, and the test vector covers byte-level stability.

### Envelope

```
offset  size  field
0       1     version   (1)
1       1     algorithm (1 = XChaCha20-Poly1305)
2       24    nonce
26      n+16  ciphertext || Poly1305 tag
```

AAD is `"notebook/v1/aad" || version || algorithm || vault_id (16) || kind (1) || note_id (16, zero for vault-level kinds)`. All fields are fixed-length, so the encoding is unambiguous. The header is authenticated by being in the AAD; the nonce is authenticated by the AEAD itself.

`open` checks length first (`< 42` → `Malformed`), then version (`UnsupportedVersion(v)`), then algorithm (`UnsupportedAlgorithm(a)`), then decrypts. Every authentication failure is `CryptoError::Decrypt`, with no detail. Reporting unknown versions separately is safe because the header is public, and it lets an old client say "update the app" instead of "corrupt data".

Kinds (one byte, part of the AAD):

| kind | value | note id | key |
|------|-------|---------|-----|
| `NotePayload` | 1 | yes | note key |
| `CrdtUpdate` | 2 | yes | note key |
| `CrdtSnapshot` | 3 | yes | note key |
| wrapped note key | 16 | yes | vault key |
| bundle: vault key under auth secret | 32 | no | derived |
| bundle: vault key under recovery key | 33 | no | derived |

Only the first three are public (`PayloadKind`); the rest are internal so callers can't encrypt user data under a key-wrapping context.

### Key types

- A private `SymmetricKey([u8; 32])` with `Zeroize`/`ZeroizeOnDrop`, wrapped by `VaultKey`, `NoteKey`, and the internal `WrappingKey`. None implement `Clone` or expose bytes publicly; `Debug` prints the type name and `..`.
- `NoteKey::encrypt(vault, note, kind, plaintext) -> Result<Vec<u8>>` and `decrypt(...) -> Result<Zeroizing<Vec<u8>>>`.
- `VaultKey::wrap_note_key(vault, note, &NoteKey) -> Result<Vec<u8>>` and `unwrap_note_key(vault, note, &[u8]) -> Result<NoteKey>`.
- `VaultId` is a ULID newtype mirroring `NoteId` (`from_parts`, `Display`, `FromStr`). The client creates it at registration so the bundle can be bound to it before upload.

### Auth secret and wrapping-key derivation

`AuthSecret::from_bytes(&[u8])` takes at least 32 bytes (OPAQUE's export key is 64) and stores them zeroized. The wrapping key is `HKDF-SHA256(salt = vault_id, ikm = secret, info = "notebook/v1/kek/auth")`. The recovery key goes through the same derivation with `info = "notebook/v1/kek/recovery"`. Because both the KDF salt and the AAD include the vault ID, a wrapped entry moved to another vault's bundle cannot be unwrapped.

HKDF rather than Argon2 for these: both inputs already carry at least 128 bits of entropy, so stretching adds cost (especially in the browser) without adding security.

### Recovery key

20 bytes: 16 random bytes plus the first 4 bytes of `SHA-256("notebook/v1/recovery" || random)`. 160 bits encode to exactly 32 Crockford base32 characters, displayed as 8 groups of 4 (`ABCD-EFGH-...`). The checksum catches typos before a decryption attempt (the spec requires a distinct error), and with a 32-bit checksum a random typo passes with probability 2^-32. Only the 16 random bytes feed HKDF. `Display` is not implemented; `RecoveryKey::to_display() -> Zeroizing<String>` makes showing it a deliberate act.

### Key bundle

```
offset  size  field
0       1     bundle format version (1)
1       16    vault id
17      2     auth entry length (u16 big-endian)
19      a     auth entry (envelope, kind 32)
19+a    2     recovery entry length
21+a    r     recovery entry (envelope, kind 33)
```

`KeyBundle::create(vault_id, &VaultKey, &AuthSecret, &RecoveryKey)`, `unlock_with_auth`, `unlock_with_recovery`, `rewrap_auth(&VaultKey, &AuthSecret)`, `vault_id()`, `to_bytes()`, `from_bytes()`. Parsing checks lengths exactly and rejects trailing bytes. Two fixed slots instead of a list of entries keeps parsing trivial; a format version bump can add slots later.

`rewrap_auth` takes the already-unlocked `VaultKey` rather than the old secret so recovery-then-new-passphrase uses the same call.

### Passphrase derivation

`KdfVersion::V1` maps to Argon2id, m = 65536 KiB, t = 3, p = 1, 32-byte output, which is RFC 9106's second recommended option with p reduced to 1 because browsers run it single-threaded. `derive_auth_secret(passphrase, &Salt, KdfVersion) -> Result<AuthSecret>`; `Salt::generate()` gives 16 random bytes; `KdfVersion::from_u8` rejects unknown numbers so stored parameters can't be downgraded. Nothing in this change stores a passphrase-derived bundle yet; the function exists so any direct-passphrase path (for example a future local unlock) uses vetted settings.

Argon2 at 64 MiB is very slow in unoptimized builds, so the workspace sets `[profile.dev.package.argon2] opt-level = 3` (and for `blake2`, its hash), keeping `cargo test` fast without changing release behavior.

## Risks / Trade-offs

- [New, recently released crate versions (`chacha20poly1305` 0.11, `aead` 0.6)] → Fixed test vectors pin the output, so a dependency change that alters bytes fails the test.
- [Zeroization is best-effort: `Vec` reallocation and compiler copies can leave key bytes behind] → Keys live in fixed-size arrays that never reallocate; plaintext buffers are `Zeroizing<Vec<u8>>`. This matches what's achievable in safe Rust.
- [No rollback protection] → Recorded in the threat model; the sync protocol handles ordering.
- [Recovery key holds 128 bits of randomness, not 256] → 128-bit keys are beyond brute force, and a 32-character code is something people can actually write down.
- [Argon2 64 MiB in a browser tab can be slow on low-end phones] → Only used on direct-passphrase paths; the normal login uses the OPAQUE export key through HKDF.
