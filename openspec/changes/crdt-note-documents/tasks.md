# Tasks

## 1. Dependencies and module skeleton

- [ ] 1.1 Add `yrs = "0.28"` and `dissimilar` to `[workspace.dependencies]` and `notebook-core`; create an empty `document` module (`document/{mod,format,doc}.rs`) re-exported from `lib.rs`; verify `cargo build -p notebook-core` and `cargo build -p notebook-core --target wasm32-unknown-unknown` both succeed (check that `fastrand`'s `js` feature resolves `getrandom` 0.2 on wasm without extra target config)
- [ ] 1.2 Add a `pub(crate) Note::from_parts(id, body, created, updated, pinned, archived, trashed)` constructor to `note.rs`; verify a unit test that a note built from parts reports every field and the derived title, and that existing `note` tests still pass

## 2. Format and errors

- [ ] 2.1 Implement `DocumentError { Crypto(CryptoError), UnsupportedFormat(u8), Malformed }` with `Display`/`Error`, the `FORMAT_V1 = 1` constant, and `encode_plaintext`/`decode_plaintext` that add and check the format byte (empty input is `Malformed`, unknown byte is `UnsupportedFormat`); verify unit tests for each case
- [ ] 2.2 Implement `EncryptedUpdate` and `EncryptedSnapshot` newtypes (`as_bytes`, `into_bytes`) and the encrypt/decrypt helpers over `NoteKey` with `PayloadKind::CrdtUpdate` / `CrdtSnapshot`, keeping decrypted bytes in `Zeroizing`; verify unit tests that an update does not decrypt as a snapshot and vice versa

## 3. Document creation, local edits, and current state

- [ ] 3.1 Implement `NoteDocument` (vault, note ID, owned `NoteKey`, `yrs::Doc` with `OffsetKind::Utf16` and a random 32-bit client ID; `pub(crate)` fixed-client-ID constructor for tests), `open`, `create` (writes `body` text and `meta` `created`/`updated`/flags in one transaction and returns its encrypted update), `id`, `vault_id`, and redacted `Debug`; verify tests for "New document matches the note it was created from" and "Creation produces an initial update", and that `Debug` output contains no key bytes or body text
- [ ] 3.2 Implement `note()` with tolerant `meta` reading (`None` without `created`; non-bool flags read `false`; non-integer `updated` reads `created`; `Any::Number` accepted for timestamps); verify tests for "Title derived from the merged body", "Unreadable metadata falls back to defaults", and "Document missing its creation update"
- [ ] 3.3 Implement `set_pinned`/`set_archived`/`set_trashed` returning `Option<EncryptedUpdate>` from the transaction's `encode_update_v1`, with no transaction on a no-op; verify tests for "Flag change yields an update", the flag half of "No-op change yields nothing", and "Flag change leaves updated time alone"
- [ ] 3.4 Implement `set_body(new, now)` with a `dissimilar` char diff applied as UTF-16 deletes and inserts back to front in one transaction that also sets `meta.updated = now`, with no transaction when the text is unchanged; verify tests for "Body edit yields an update", the body half of "No-op change yields nothing", "Body edit bumps updated time", round trips of CRLF, emoji (surrogate pairs), and combining characters, and a property test that `set_body` always produces exactly the target text

## 4. Applying remote updates and snapshots

- [ ] 4.1 Implement `apply_update`: decrypt, check format, `Update::decode_v1` before opening a transaction, then apply, mapping Yrs `UpdateError` to `Malformed`; verify tests for "Update from another note is rejected", "Tampered update", "Garbage that decrypts", and "Unknown document format", each asserting `note()` and the encoded state are unchanged afterwards
- [ ] 4.2 Verify merge behavior with multi-replica tests: "Concurrent edits to different lines", "Setting the body keeps concurrent edits between changed spans", "Concurrent inserts at the same place", "Any order" (three replicas, two permutations), "Duplicates are harmless", "Concurrent flag changes", and "Out-of-order delivery"
- [ ] 4.3 Implement `snapshot()` (full state via `encode_state_as_update_v1` against an empty state vector, including held updates) and `load_snapshot` with the same decrypt/format/decode-first checks as `apply_update`; verify tests for "Snapshot alone", "Snapshot plus later updates", "Overlapping updates", "Snapshot keeps held updates", "Update presented as a snapshot is rejected", and "Ciphertext hides the body" (for both an update and a snapshot)
- [ ] 4.4 Add a property test that applies random byte flips, truncations, and random plaintext (encrypted with the right key and format byte) through `apply_update` and `load_snapshot`; verify it never panics and the document state is unchanged on every error (if it finds a partial apply, switch untrusted applies to a scratch-clone and note it in the module docs)

## 5. Format pinning and documentation

- [ ] 5.1 Record a test vector: a document created with a fixed client ID, vault, note ID, body, and time; assert its initial update's plaintext (format byte plus Yrs v1 bytes) matches the recorded bytes; verify the "Fixed output for fixed input" test passes on native and that the same assertion runs under `cargo test` for the core crate
- [ ] 5.2 Write rustdoc for the `document` module: root names (`body`, `meta`) and `meta` keys as a fixed format, the format byte, update vs snapshot, client-ID rules, and the ciphertext-only API; verify `cargo doc -p notebook-core --no-deps` builds without warnings

## 6. Integration checks

- [ ] 6.1 Run `scripts/check.sh` and verify it exits 0 (fmt, clippy with `-D warnings`, workspace tests, wasm32 core build)
- [ ] 6.2 Run `openspec validate crdt-note-documents --strict` and verify it passes

## Workflow follow-up

- Review the change (including a `security-analyst` pass, since it defines encrypted payloads), then archive it with `/opsx:archive` so the `note-documents` spec lands in `openspec/specs/`.
- Update `TODO.md`: mark `crdt-note-documents` as `[x]` once archived.
