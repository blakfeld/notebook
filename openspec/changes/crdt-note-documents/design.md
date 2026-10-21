# Design

## Context

See proposal.md (Why). `notebook-core` already has `note` (the `Note` value, `NoteId`, `Timestamp`, title derivation) and `crypto` (`VaultId`, `NoteKey`, `PayloadKind::{CrdtUpdate, CrdtSnapshot}`, the envelope). The `note` module takes time from the caller; the `crypto` module draws its own randomness to remove nonce-reuse footguns. The crate forbids `unsafe` and must build for `wasm32-unknown-unknown`. Requirements are in `specs/note-documents/spec.md`.

Downstream consumers fix the constraints: `local-storage` persists the ciphertexts this change emits, `sync-server` relays them as opaque blobs, and `note-editor-component` plans to bind CodeMirror through `yjs` + `y-codemirror.next`. The bytes defined here are long-lived.

## Goals / Non-Goals

**Goals:**
- One `NoteDocument` type that is the only way to read or change a note's content, and that only ever emits ciphertext.
- A wire format pinned by a test vector and prefixed by a version byte, so it can change later without a flag day.

**Non-Goals:**
- How the editor's live document crosses the Tauri/WASM boundary, and whether JS or Rust owns it while a note is open. That's `note-editor-component`'s design. That change adds the plaintext editor exchange to this capability. It is a distinct `EditorUpdate` type in plain Yjs v1, with no format byte, and never goes to storage or sync. This change doesn't expose it.
- Undo/redo (the editor scopes undo to local edits).
- Garbage collection tuning, update merging/squashing, and deciding *when* to snapshot (`sync-client-engine`).
- Storage, sync ordering, permanent purge.

## Decisions

### Yrs, not Loro

`yrs` 0.28. It is Yjs's Rust port and shares its binary update format, so updates made in Rust apply in `yjs` and vice versa. `y-codemirror.next` is the mature CodeMirror 6 binding, with remote cursors and local-only undo built in. Yrs builds for wasm32 (it backs `ywasm`).

**Alternative considered: Loro.** It has better rich-text and move semantics and a smaller snapshot format. But its CodeMirror binding is younger, and we'd give up the option of running plain `yjs` in the webview. A markdown note is a single text plus a few flags, so we don't need Loro's richer types.

### Document layout

One Yrs `Doc` per note, with two root types:

| root | type | contents |
|------|------|----------|
| `body` | `Text` | markdown body |
| `meta` | `Map` | `created` (i64 ms), `updated` (i64 ms), `pinned`, `archived`, `trashed` (bool) |

Root names are part of the format and must never be renamed. New fields are added as new `meta` keys; readers ignore keys they don't know. `created` is written only by `create`. Concurrent writes to the same `meta` key resolve last-writer-wins by Yrs's deterministic rule (higher client ID wins on a true conflict). For flags this just has to converge, and it does. For `updated` it means concurrent edits can settle on the slightly earlier of the two times. Nothing depends on `updated` beyond sorting, so we accept that.

Text offsets: `Options::offset_kind = OffsetKind::Utf16`, matching Yjs and CodeMirror's JS string indices, so offsets agree if the bridge ever passes positions. The Rust API itself never exposes offsets (see "Editing API").

### Client IDs are drawn inside the module

Each `NoteDocument` instance gets a fresh random client ID, drawn with the crate's existing secure-random helper and masked to 32 bits, which is what Yjs generates, so the IDs stay exact as JS numbers if updates ever reach `yjs`. Two live replicas that share a client ID and have diverged corrupt each other irrecoverably, so callers aren't allowed to pick one. As in `crypto`, a `pub(crate)` constructor takes a fixed client ID for the test vector. A new ID on every open costs a few bytes of state-vector growth per session, which compaction absorbs.

**Alternative considered:** a persisted per-device client ID. Rejected for now: it saves bytes but brings back the reuse hazard (restored backups, a crash before the outbox flushes). Revisit if state vectors grow noticeably.

### Encoding and versioning

Plaintext of every update and snapshot = `format (1 byte) || payload`. Format `1` = Yrs/Yjs **update v1** encoding (`encode_v1` / `Update::decode_v1`). Snapshots are a full-state update (`encode_state_as_update_v1` against an empty state vector) rather than Yrs's `Snapshot` type (which is a state vector plus delete set, for history views, not for restoring state). That keeps one decoder for both, and lets "snapshot + overlapping updates" fall out of CRDT idempotence. In Yrs 0.28, `encode_state_as_update_v1` also merges in the pending (held) structs and delete set, so a snapshot taken while an update waits on a missing dependency still carries that update; a test pins this, because compaction would otherwise drop data.

v1 over v2: it's Yjs's default, and every Yjs tool and provider speaks it. v2 is smaller, but the gain is modest after compaction, and v2 can become format `2` later without breaking anything.

That plaintext then goes through `NoteKey::encrypt(vault, note, PayloadKind::CrdtUpdate | CrdtSnapshot, …)`. The kind in the AAD is what separates an update from a snapshot. The format byte doesn't do that job.

### API shape

```rust
pub struct NoteDocument { /* vault, id, NoteKey, yrs::Doc */ }
pub struct EncryptedUpdate(Vec<u8>);    // as_bytes / into_bytes
pub struct EncryptedSnapshot(Vec<u8>);

impl NoteDocument {
    pub fn create(vault, id, key: NoteKey, body, now) -> Result<(Self, EncryptedUpdate)>;
    pub fn open(vault, id, key: NoteKey) -> Result<Self>;   // empty replica
    pub fn load_snapshot(&mut self, &[u8]) -> Result<()>;
    pub fn apply_update(&mut self, &[u8]) -> Result<()>;
    pub fn set_body(&mut self, body, now) -> Result<Option<EncryptedUpdate>>;
    pub fn set_pinned / set_archived / set_trashed(&mut self, bool) -> Result<Option<EncryptedUpdate>>;
    pub fn snapshot(&self) -> Result<EncryptedSnapshot>;
    pub fn note(&self) -> Option<Note>;
    pub fn id(&self) -> NoteId; pub fn vault_id(&self) -> VaultId;
}
```

- The document **owns** its `NoteKey` (keys aren't `Clone`). Storage or sync unwraps the note key once and hands it over. Every mutator defined here returns ciphertext, so nothing this change adds hands plaintext to storage or sync. The only plaintext path is the editor exchange that `note-editor-component` adds (`EditorUpdate`, `editor_attach`, `apply_editor_update`, `editor_pending`). It is a distinct type, and its `apply_editor_update` returns an `EncryptedUpdate` for sync. `Debug` is redacted.
- Each local edit runs in one `TransactionMut`, and the returned update is that transaction's own `encode_update_v1()` (only its inserts and deletes), not `encode_diff_v1`, which would repeat the whole delete set every time. Mutators compare against the current value first and open no transaction for a no-op, which is how "no-op change yields nothing" is met.
- `note()` builds a `notes::Note` through a new `pub(crate) Note::from_parts`, so title and tag derivation stay in one place. It returns `None` until `meta.created` is present. Reading `meta` is tolerant: a flag that isn't a bool reads as `false`, an `updated` that isn't an integer reads as `created`, unknown keys are ignored. Only the user's own clients hold the note key, so a wrongly typed value is a bug, not an attack, and it must not make the note unreadable. Timestamps are written as `Any::BigInt(i64)`; readers also accept `Any::Number` holding an integral value, since a JS `yjs` writer would produce that.
- `DocumentError { Crypto(CryptoError), UnsupportedFormat(u8), Malformed }`. The crypto variant passes through decryption and unsupported-envelope errors unchanged.
- `sync-client-engine` later adds `NoteDocument::full_update() -> EncryptedUpdate` for its server-restore resync: the same format-1 full-state bytes as `snapshot()`, encrypted as `CrdtUpdate` instead of `CrdtSnapshot`. It uses only the formats defined here and changes no `note-documents` requirement.

### Editing API: whole-body set with a minimal diff

`set_body(new, now)` diffs the current text against `new` and applies the result as UTF-16-correct deletes and inserts in one transaction. The diff is a character-level Myers diff from the `dissimilar` crate (dtolnay; no dependencies, pure Rust, builds for wasm32). Its `Equal`/`Delete`/`Insert` chunks are walked in order, converting `char` counts to UTF-16 offsets, and applied back to front so earlier offsets stay valid. Because chunks are whole `char`s, a surrogate pair is never split. Several separate changes become several small edits, so concurrent edits between them survive. That matters for tag rename, which rewrites every occurrence of a tag in a body.

**Alternative considered:** a common-prefix/common-suffix trim (one replaced span, no dependency). Rejected: when a body changes in two places, everything between them is deleted and reinserted, which wipes out a concurrent edit in that middle region. Tag rename hits exactly that case. The editor binding will feed the shared text directly with per-keystroke changes, so `set_body` is for non-editor callers: tag rename, imports, tests.

**Alternative considered:** expose `splice(offset, delete, insert)`. That means picking an offset unit (UTF-8 vs UTF-16) in a public Rust API before the bridge design exists. Deferred to `note-editor-component`.

Tag rename (already implemented over `Note`) will call `set_body` with the rewritten body, which turns into one small edit per renamed occurrence. Wiring that up belongs to the app/sync changes.

### Error atomicity

`apply_update` / `load_snapshot` decrypt, check the format byte, then `Update::decode_v1` *before* opening a transaction. Decode is where malformed input fails, so the document is untouched. Yrs's `apply_update` can still return `UpdateError` after decoding, for structurally valid but inconsistent updates. Those are mapped to `Malformed`. To keep the "state unchanged" guarantee there too, we test it with the fuzz-style tests in tasks rather than cloning the doc on every apply. If a failing case turns up, we fall back to applying into a scratch clone for untrusted input.

Updates missing their dependencies are kept in Yrs's pending store and integrate automatically once the gap fills. That's the out-of-order requirement, at no extra cost.

## Risks / Trade-offs

- [Long-lived format tied to Yrs's encoding] → It's the Yjs v1 wire format, which is stable and widely implemented. The format byte reserves a migration path, and a test vector pins the bytes so a Yrs upgrade that changes output fails CI.
- [Yrs pulls in `fastrand` with `js`, i.e. a second `getrandom` (0.2) on wasm, plus `serde_json`, `dashmap`, `async-lock`] → Task 1 verifies the wasm32 build. Our own randomness still goes through `getrandom` 0.4. `fastrand` is only used where Yrs would pick a client ID itself, and we always supply one.
- [LWW `updated` can settle on the earlier of two concurrent times] → Accepted (sorting only). If it matters later, a per-client max can be added as a new `meta` key.
- [`apply_update` partial-apply on inconsistent input] → Decode first, fuzz-test the state-unchanged property, scratch-clone fallback if needed.
- [Fresh client ID per open grows state vectors] → Snapshots compact them. Revisit with a persisted device ID if measurements warrant it.
- [Metadata leaks: update count, size, and timing per note] → Already accepted in the vault-crypto threat model.
- [Decrypted body lives in the Yrs doc and can't be zeroized on drop] → Same exposure as the plaintext `Note` the app already holds. Decrypted update and snapshot bytes are kept in `Zeroizing` buffers until decoded.
- [`dissimilar` diff cost on large bodies] → Myers is O(ND); notes are small and `set_body` is off the keystroke path. Revisit only if tag rename across a large vault is measurably slow.

## Migration Plan

None. No notes are persisted yet. This change defines format `1`.

## Open Questions

None that change what this change builds. The plaintext editor exchange is specified by `note-editor-component` as ADDED requirements on this capability. It doesn't change the formats defined here.
