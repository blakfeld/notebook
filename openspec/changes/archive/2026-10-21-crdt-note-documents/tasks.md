# Tasks

## 1. Dependencies and module skeleton

- [x] 1.1 Add `yrs = "0.28"` and `dissimilar` to `[workspace.dependencies]` and `notebook-core`; create an empty `document` module (`document/{mod,format,doc}.rs`) re-exported from `lib.rs`; verify `cargo build -p notebook-core` and `cargo build -p notebook-core --target wasm32-unknown-unknown` both succeed (check that `fastrand`'s `js` feature resolves `getrandom` 0.2 on wasm without extra target config)
- [x] 1.2 Add a `pub(crate) Note::from_parts(id, body, created, updated, pinned, archived, trashed)` constructor to `note.rs`; verify a unit test that a note built from parts reports every field and the derived title, and that existing `note` tests still pass

## 2. Format and errors

- [x] 2.1 Implement `DocumentError { Crypto(CryptoError), UnsupportedFormat(u8), Malformed }` with `Display`/`Error`, the `FORMAT_V1 = 1` constant, and `encode_plaintext`/`decode_plaintext` that add and check the format byte (empty input is `Malformed`, unknown byte is `UnsupportedFormat`); verify unit tests for each case
- [x] 2.2 Implement `EncryptedUpdate` and `EncryptedSnapshot` newtypes (`as_bytes`, `into_bytes`) and the encrypt/decrypt helpers over `NoteKey` with `PayloadKind::CrdtUpdate` / `CrdtSnapshot`, keeping decrypted bytes in `Zeroizing`; verify unit tests that an update does not decrypt as a snapshot and vice versa

## 3. Document creation, local edits, and current state

- [x] 3.1 Implement `NoteDocument` (vault, note ID, owned `NoteKey`, `yrs::Doc` with `OffsetKind::Utf16` and a random 32-bit client ID; `pub(crate)` fixed-client-ID constructor for tests), `open`, `create` (writes `body` text and `meta` `created`/`updated`/flags in one transaction and returns its encrypted update), `id`, `vault_id`, and redacted `Debug`; verify tests for "New document matches the note it was created from" and "Creation produces an initial update", and that `Debug` output contains no key bytes or body text
- [x] 3.2 Implement `note()` with tolerant `meta` reading (`None` without `created`; non-bool flags read `false`; non-integer `updated` reads `created`; `Any::Number` accepted for timestamps); verify tests for "Title derived from the merged body", "Unreadable metadata falls back to defaults", and "Document missing its creation update"
- [x] 3.3 Implement `set_pinned`/`set_archived`/`set_trashed` returning `Option<EncryptedUpdate>` from the transaction's `encode_update_v1`, with no transaction on a no-op; verify tests for "Flag change yields an update", the flag half of "No-op change yields nothing", and "Flag change leaves updated time alone"
- [x] 3.4 Implement `set_body(new, now)` with a `dissimilar` char diff applied as UTF-16 deletes and inserts back to front in one transaction that also sets `meta.updated = now`, with no transaction when the text is unchanged; verify tests for "Body edit yields an update", the body half of "No-op change yields nothing", "Body edit bumps updated time", round trips of CRLF, emoji (surrogate pairs), and combining characters, and a property test that `set_body` always produces exactly the target text

## 4. Applying remote updates and snapshots

- [x] 4.1 Implement `apply_update`: decrypt, check format, `Update::decode_v1` before opening a transaction, then apply, mapping Yrs `UpdateError` to `Malformed`; verify tests for "Update from another note is rejected", "Tampered update", "Garbage that decrypts", and "Unknown document format", each asserting `note()` and the encoded state are unchanged afterwards
- [x] 4.2 Verify merge behavior with multi-replica tests: "Concurrent edits to different lines", "Setting the body keeps concurrent edits between changed spans", "Concurrent inserts at the same place", "Any order" (three replicas, two permutations), "Duplicates are harmless", "Concurrent flag changes", and "Out-of-order delivery"
- [x] 4.3 Implement `snapshot()` (full state via `encode_state_as_update_v1` against an empty state vector, including held updates) and `load_snapshot` with the same decrypt/format/decode-first checks as `apply_update`; verify tests for "Snapshot alone", "Snapshot plus later updates", "Overlapping updates", "Snapshot keeps held updates", "Update presented as a snapshot is rejected", and "Ciphertext hides the body" (for both an update and a snapshot)
- [x] 4.4 Add a property test that applies random byte flips, truncations, and random plaintext (encrypted with the right key and format byte) through `apply_update` and `load_snapshot`; verify it never panics and the document state is unchanged on every error (if it finds a partial apply, switch untrusted applies to a scratch-clone and note it in the module docs)

## 5. Format pinning and documentation

- [x] 5.1 Record a test vector: a document created with a fixed client ID, vault, note ID, body, and time; assert its initial update's plaintext (format byte plus Yrs v1 bytes) matches the recorded bytes; verify the "Fixed output for fixed input" test passes on native and that the same assertion runs under `cargo test` for the core crate
- [x] 5.2 Write rustdoc for the `document` module: root names (`body`, `meta`) and `meta` keys as a fixed format, the format byte, update vs snapshot, client-ID rules, and the ciphertext-only API; verify `cargo doc -p notebook-core --no-deps` builds without warnings

## 6. Integration checks

- [x] 6.1 Run `scripts/check.sh` and verify it exits 0 (fmt, clippy with `-D warnings`, workspace tests, wasm32 core build)
- [x] 6.2 Run `openspec validate crdt-note-documents --strict` and verify it passes

## 7. Review fixes

- [x] 7.1 Add `document/validate.rs`: a structural scan of format-1 payloads before `Update::decode_v1` that rejects as `Malformed` any count larger than the remaining bytes, any client clock or delete range past `u32::MAX`, explicit-ID parents, root names other than `body`/`meta`, content other than String/Any/Deleted, Any values nested deeper than a fixed limit, trailing bytes, and plaintext above `MAX_PLAINTEXT_LEN` (16 MiB); verify regression tests for the reviewers' cases (partial apply, held bad update poisoning a valid one, clock overflow via Skip/GC/delete set/string item, the 6-byte allocation bomb, trailing bytes, foreign root, oversized input) and that every update and snapshot the document itself emits passes the scan
- [x] 7.2 Keep `set_body` exact when `body` holds non-string content: embeds and nested types are rejected by the structural check, and a value in `body` (counted as an offset by Yrs insertion but skipped by removal) makes `set_body` fall back to removing all string content and inserting the target at offset 0; a debug assertion checks the result equals the target; verify a test with a value in `body`
- [x] 7.3 Keep encoded update and snapshot payloads in `Zeroizing` buffers in `create`, `set_body`, `set_flag`, and `snapshot`
- [x] 7.4 Rotate the client ID (rebuild the doc from its encoded state under a fresh random ID) after an apply that brings in content under this instance's ID beyond what it wrote; verify a client-ID collision test converges
- [x] 7.5 Extend the randomized bad-input unit test (`bad_input_never_panics_or_changes_state`) with structured updates (valid headers with extreme clocks, lengths, counts, and parent IDs) and fix its case 0 so update ciphertext is mutated for update applies; then re-check 4.4
- [x] 7.6 Update `design.md` (error atomicity via pre-decode scan, size cap, schema-restricted content, client-ID rotation, deferred risks) and add spec scenarios for oversized input, out-of-schema content, and client-ID collision; verify `openspec validate crdt-note-documents --strict`
- [x] 7.7 Run `scripts/check.sh` and verify it exits 0; re-run the code-reviewer and security-analyst agents

## 8. Second review: Yrs robustness

- [x] 8.1 Close the validator gaps: clocks and clock ranges at most `i32::MAX`; `Any` integers skipped by a hand-written varint reader that rejects overlong or overflowing encodings; `Any` map/array counts read as `u64` and bounded by the remaining bytes; zero-length GC, Skip, String, Any, and Deleted blocks rejected; a client repeated in the block section or the delete set rejected (compared as the `u32` Yrs keeps); delete ranges sorted, non-overlapping, and non-empty; verify regression tests for each reviewer payload
- [x] 8.2 Vendor Yrs 0.28.0 under `vendor/yrs` (patched in through `[patch.crates-io]`, excluded from the workspace) and fix its memory-safety and panic bugs: integration no longer overwrites a non-Skip block (a block below the local clock is integrated only when its whole range lies in a skip, and dropped otherwise), clock offsets are computed without `i32` overflow, `BlockSet::exclude` no longer unwraps an empty block list, and `Block::splice` leaves no neighbour links on unintegrated halves (a use-after-free found while fuzzing; dedicated fuzzing was then dropped as unnecessary for a personal app); verify the reviewers' crash payloads are rejected or applied without panic both with and without the validator
- [x] 8.3 Update `design.md` (vendored Yrs and its patches, residual risk for key holders), the module docs, and the proposal where claims changed; draft the upstream y-crdt issue text for the user to file
- [x] 8.4 Retry held updates in the vendored Yrs whenever new blocks arrive, so a held update whose dependency fills an integrated skip integrates (found in the final review: replicas diverged on ordinary out-of-order delivery); verify `held_update_integrates_when_a_skip_fills` and `shuffled_delivery_converges` fail without the patch and pass with it
- [x] 8.5 Run `scripts/check.sh` and `openspec validate --strict`, and a final quick code-reviewer pass (a full security round is unnecessary for a personal app)

## Workflow follow-up

- Review the change (including a `security-analyst` pass, since it defines encrypted payloads), then archive it with `/opsx:archive` so the `note-documents` spec lands in `openspec/specs/`.
- Update `TODO.md`: mark `crdt-note-documents` as `[x]` once archived.
