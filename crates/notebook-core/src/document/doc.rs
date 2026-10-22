//! The note document: a Yrs doc bound to one note and its key.

use std::fmt;

use dissimilar::Chunk;
use yrs::types::GetString;
use yrs::updates::decoder::Decode;
use yrs::{
    Any, ClientID, Doc, Map, MapRef, Number, OffsetKind, Options, Out, ReadTxn, StateVector, Text,
    TextRef, Transact, TransactionMut, Update, WriteTxn,
};
use zeroize::Zeroizing;

use super::format::{
    DocumentError, EncryptedSnapshot, EncryptedUpdate, decode_plaintext, decrypt, encrypt_snapshot,
    encrypt_update,
};
use super::validate::{MAX_PLAINTEXT_LEN, check_update};
use crate::crypto::{ENVELOPE_OVERHEAD, NoteKey, PayloadKind, VaultId, fill_random};
use crate::note::{Note, NoteId, Timestamp};

/// Root `Text` holding the markdown body.
pub(crate) const ROOT_BODY: &str = "body";
/// Root `Map` holding the note's metadata.
pub(crate) const ROOT_META: &str = "meta";

pub(crate) const META_CREATED: &str = "created";
pub(crate) const META_UPDATED: &str = "updated";
pub(crate) const META_PINNED: &str = "pinned";
pub(crate) const META_ARCHIVED: &str = "archived";
pub(crate) const META_TRASHED: &str = "trashed";

/// One note's CRDT document.
///
/// It owns the note's key, and every change it makes or accepts crosses its
/// API only as ciphertext. See the [module docs](super) for the format.
pub struct NoteDocument {
    vault: VaultId,
    id: NoteId,
    key: NoteKey,
    doc: Doc,
    body: TextRef,
    meta: MapRef,
    /// The clock this instance has reached under its own client ID. More
    /// than this under our ID means another replica drew the same ID.
    own_clock: u32,
}

impl NoteDocument {
    /// Opens an empty replica of note `id`, ready to load a snapshot or apply
    /// updates.
    pub fn open(vault: VaultId, id: NoteId, key: NoteKey) -> Result<Self, DocumentError> {
        Ok(Self::with_client_id(vault, id, key, random_client_id()?))
    }

    /// Creates the document for a new note with `body` at `now`, all flags
    /// cleared. Also returns the encrypted update that carries the whole
    /// creation, for storage and sync.
    pub fn create(
        vault: VaultId,
        id: NoteId,
        key: NoteKey,
        body: &str,
        now: Timestamp,
    ) -> Result<(Self, EncryptedUpdate), DocumentError> {
        Self::create_with_client_id(vault, id, key, body, now, random_client_id()?)
    }

    /// [`open`](Self::open) with a fixed client ID, for test vectors and
    /// tests that need a deterministic tie-break.
    pub(crate) fn with_client_id(vault: VaultId, id: NoteId, key: NoteKey, client_id: u32) -> Self {
        let (doc, body, meta) = new_doc(client_id);
        Self {
            vault,
            id,
            key,
            doc,
            body,
            meta,
            own_clock: 0,
        }
    }

    /// [`create`](Self::create) with a fixed client ID.
    pub(crate) fn create_with_client_id(
        vault: VaultId,
        id: NoteId,
        key: NoteKey,
        body: &str,
        now: Timestamp,
        client_id: u32,
    ) -> Result<(Self, EncryptedUpdate), DocumentError> {
        let mut document = Self::with_client_id(vault, id, key, client_id);
        let payload = document.local_edit(|doc, txn| {
            doc.body.insert(txn, 0, body);
            doc.meta.insert(txn, META_CREATED, timestamp(now));
            doc.meta.insert(txn, META_UPDATED, timestamp(now));
            doc.meta.insert(txn, META_PINNED, false);
            doc.meta.insert(txn, META_ARCHIVED, false);
            doc.meta.insert(txn, META_TRASHED, false);
        });
        let update = document.encrypt_update(&payload)?;
        Ok((document, update))
    }

    pub fn id(&self) -> NoteId {
        self.id
    }

    pub fn vault_id(&self) -> VaultId {
        self.vault
    }

    /// The current note, or `None` until the document has received the
    /// update that created it.
    ///
    /// Metadata of the wrong type reads as its default: a flag as `false`,
    /// the updated time as the created time.
    pub fn note(&self) -> Option<Note> {
        let txn = self.doc.transact();
        let created = read_timestamp(self.meta.get(&txn, META_CREATED))?;
        let updated = read_timestamp(self.meta.get(&txn, META_UPDATED)).unwrap_or(created);
        let flag = |key| matches!(self.meta.get(&txn, key), Some(Out::Any(Any::Bool(true))));
        Some(Note::from_parts(
            self.id,
            self.body.get_string(&txn),
            created,
            updated,
            flag(META_PINNED),
            flag(META_ARCHIVED),
            flag(META_TRASHED),
        ))
    }

    /// Sets the body to `body`, changing only the spans that differ, and the
    /// updated time to `now`. Returns `None` if the body is already `body`.
    pub fn set_body(
        &mut self,
        body: &str,
        now: Timestamp,
    ) -> Result<Option<EncryptedUpdate>, DocumentError> {
        let (current, only_text) = {
            let txn = self.doc.transact();
            let current = Zeroizing::new(self.body.get_string(&txn));
            // A value in the body (which only a foreign writer could put
            // there) is invisible to `get_string` and to removal, but counts
            // as an offset for insertion, so diff offsets would drift.
            let only_text = self.body.len(&txn) == utf16_len(&current);
            (current, only_text)
        };
        if *current == body {
            return Ok(None);
        }
        let payload = self.local_edit(|doc, txn| {
            if only_text {
                // Chunks are whole `char`s, so a surrogate pair is never
                // split. Walking front to back, `pos` is the UTF-16 offset
                // in the text as edited so far.
                let mut pos = 0;
                for chunk in dissimilar::diff(&current, body) {
                    match chunk {
                        Chunk::Equal(text) => pos += utf16_len(text),
                        Chunk::Delete(text) => doc.body.remove_range(txn, pos, utf16_len(text)),
                        Chunk::Insert(text) => {
                            doc.body.insert(txn, pos, text);
                            pos += utf16_len(text);
                        }
                    }
                }
            } else {
                // Removal skips values, and offset 0 is the start either
                // way, so this is exact at the cost of one big edit.
                doc.body.remove_range(txn, 0, utf16_len(&current));
                doc.body.insert(txn, 0, body);
            }
            debug_assert_eq!(doc.body.get_string(txn), body);
            doc.meta.insert(txn, META_UPDATED, timestamp(now));
        });
        self.encrypt_update(&payload).map(Some)
    }

    /// Pins or unpins the note. Returns `None` if nothing changed.
    pub fn set_pinned(&mut self, pinned: bool) -> Result<Option<EncryptedUpdate>, DocumentError> {
        self.set_flag(META_PINNED, pinned)
    }

    /// Archives or unarchives the note. Returns `None` if nothing changed.
    pub fn set_archived(
        &mut self,
        archived: bool,
    ) -> Result<Option<EncryptedUpdate>, DocumentError> {
        self.set_flag(META_ARCHIVED, archived)
    }

    /// Trashes or restores the note. Returns `None` if nothing changed.
    pub fn set_trashed(&mut self, trashed: bool) -> Result<Option<EncryptedUpdate>, DocumentError> {
        self.set_flag(META_TRASHED, trashed)
    }

    /// Applies an encrypted update. Updates may arrive in any order, more
    /// than once, and before the updates they depend on; those are held
    /// until the gap fills. On error the document is unchanged.
    pub fn apply_update(&mut self, ciphertext: &[u8]) -> Result<(), DocumentError> {
        self.apply(PayloadKind::CrdtUpdate, ciphertext)
    }

    /// Merges an encrypted snapshot into the document. Updates before or
    /// after it can still be applied. On error the document is unchanged.
    pub fn load_snapshot(&mut self, ciphertext: &[u8]) -> Result<(), DocumentError> {
        self.apply(PayloadKind::CrdtSnapshot, ciphertext)
    }

    /// An encrypted snapshot of the full state, including updates still
    /// waiting on missing dependencies.
    pub fn snapshot(&self) -> Result<EncryptedSnapshot, DocumentError> {
        encrypt_snapshot(&self.key, self.vault, self.id, &self.full_state())
    }

    /// The same full state as [`snapshot`](Self::snapshot), encrypted as an
    /// update, so it can be pushed like any local change (the sync engine
    /// re-sends every note this way after a server restore).
    pub fn full_update(&self) -> Result<EncryptedUpdate, DocumentError> {
        self.encrypt_update(&self.full_state())
    }

    fn set_flag(
        &mut self,
        key: &str,
        value: bool,
    ) -> Result<Option<EncryptedUpdate>, DocumentError> {
        let current = matches!(
            self.meta.get(&self.doc.transact(), key),
            Some(Out::Any(Any::Bool(true)))
        );
        if current == value {
            return Ok(None);
        }
        let payload = self.local_edit(|doc, txn| {
            doc.meta.insert(txn, key, value);
        });
        self.encrypt_update(&payload).map(Some)
    }

    /// Runs one local transaction and returns its update, recording how far
    /// it moved this instance's own clock.
    fn local_edit(
        &mut self,
        edit: impl FnOnce(&Self, &mut TransactionMut<'_>),
    ) -> Zeroizing<Vec<u8>> {
        let (payload, own_clock) = {
            let mut txn = self.doc.transact_mut();
            edit(self, &mut txn);
            let own_clock = txn.state_vector().get(&self.doc.client_id());
            (Zeroizing::new(txn.encode_update_v1()), own_clock)
        };
        self.own_clock = own_clock;
        payload
    }

    fn apply(&mut self, kind: PayloadKind, ciphertext: &[u8]) -> Result<(), DocumentError> {
        if ciphertext.len() > ENVELOPE_OVERHEAD + 1 + MAX_PLAINTEXT_LEN {
            return Err(DocumentError::Malformed);
        }
        let plaintext = decrypt(&self.key, self.vault, self.id, kind, ciphertext)?;
        let payload = decode_plaintext(&plaintext)?;
        // Everything that can fail happens before the transaction opens. The
        // structural check guarantees Yrs decodes with bounded allocations
        // and that integration can't fail partway (its only error is an
        // explicit-ID parent that isn't a type, which the check rejects).
        check_update(payload).map_err(|_| DocumentError::Malformed)?;
        let update = Update::decode_v1(payload).map_err(|_| DocumentError::Malformed)?;
        let spare_client_id = random_client_id()?;
        let own_clock = {
            let mut txn = self.doc.transact_mut();
            txn.apply_update(update)
                .map_err(|_| DocumentError::Malformed)?;
            // Yrs retries held blocks only when a missing client's clock
            // moves. An update that fills a gap Yrs already skipped over
            // satisfies a held dependency without moving the clock, so the
            // held blocks would stay stuck and replicas would diverge. Take
            // them out and apply them once more; whatever still can't
            // integrate goes back to being held.
            let held = txn
                .store_mut()
                .pending_update_mut()
                .map(|pending| std::mem::take(&mut pending.update))
                .filter(|held| !held.is_empty());
            if let Some(held) = held {
                txn.apply_update(held)
                    .map_err(|_| DocumentError::Malformed)?;
            }
            txn.state_vector().get(&self.doc.client_id())
        };
        if own_clock > self.own_clock {
            self.rotate_client_id(spare_client_id);
        }
        Ok(())
    }

    /// Moves this instance to a fresh client ID after another replica turned
    /// out to share its current one, so the two don't keep corrupting each
    /// other's edits. The state, including held updates, carries over.
    fn rotate_client_id(&mut self, client_id: u32) {
        let state = self.full_state();
        let (doc, body, meta) = new_doc(client_id);
        let applied = Update::decode_v1(&state)
            .ok()
            .is_some_and(|update| doc.transact_mut().apply_update(update).is_ok());
        if applied {
            self.doc = doc;
            self.body = body;
            self.meta = meta;
            self.own_clock = 0;
        }
    }

    fn full_state(&self) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(
            self.doc
                .transact()
                .encode_state_as_update_v1(&StateVector::default()),
        )
    }

    fn encrypt_update(&self, payload: &[u8]) -> Result<EncryptedUpdate, DocumentError> {
        encrypt_update(&self.key, self.vault, self.id, payload)
    }
}

impl fmt::Debug for NoteDocument {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NoteDocument")
            .field("vault", &self.vault)
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

fn new_doc(client_id: u32) -> (Doc, TextRef, MapRef) {
    let mut options = Options::with_client_id(ClientID::new(client_id.into()));
    options.offset_kind = OffsetKind::Utf16;
    let doc = Doc::with_options(options);
    let body = doc.get_or_insert_text(ROOT_BODY);
    let meta = doc.get_or_insert_map(ROOT_META);
    (doc, body, meta)
}

/// A random 32-bit client ID, the range Yjs itself draws from.
fn random_client_id() -> Result<u32, DocumentError> {
    let mut bytes = [0; 4];
    fill_random(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn timestamp(time: Timestamp) -> Any {
    Any::Number(Number::Int(time.as_millis()))
}

/// Reads a timestamp written as an integer, or as an integral float, which
/// is how lib0 encodes integers beyond 32 bits and how a JS writer stores
/// any number.
fn read_timestamp(value: Option<Out>) -> Option<Timestamp> {
    match value? {
        Out::Any(Any::Number(Number::Int(ms))) => Some(Timestamp(ms)),
        Out::Any(Any::Number(Number::Float(ms)))
            if ms.fract() == 0.0 && ms.abs() <= Number::F64_MAX_SAFE_INTEGER =>
        {
            Some(Timestamp(ms as i64))
        }
        _ => None,
    }
}

fn utf16_len(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{CryptoError, VaultKey};
    use crate::document::FORMAT_V1;

    const T1: Timestamp = Timestamp(1_700_000_000_000);
    const T2: Timestamp = Timestamp(1_700_000_001_000);

    /// One note in one vault, with a way to hand each replica its own copy
    /// of the note key.
    struct Fixture {
        vault: VaultId,
        id: NoteId,
        vault_key: VaultKey,
        wrapped: Vec<u8>,
    }

    impl Fixture {
        fn new() -> Self {
            Self::with_ids(VaultId::from_parts(1_000, 7), NoteId::from_parts(1_000, 1))
        }

        fn for_note(n: u128) -> Self {
            Self::with_ids(VaultId::from_parts(1_000, 7), NoteId::from_parts(1_000, n))
        }

        fn with_ids(vault: VaultId, id: NoteId) -> Self {
            let vault_key = VaultKey::generate().unwrap();
            let wrapped = vault_key
                .wrap_note_key(vault, id, &NoteKey::generate().unwrap())
                .unwrap();
            Self {
                vault,
                id,
                vault_key,
                wrapped,
            }
        }

        fn key(&self) -> NoteKey {
            self.vault_key
                .unwrap_note_key(self.vault, self.id, &self.wrapped)
                .unwrap()
        }

        fn create(&self, body: &str) -> (NoteDocument, EncryptedUpdate) {
            NoteDocument::create(self.vault, self.id, self.key(), body, T1).unwrap()
        }

        fn open(&self) -> NoteDocument {
            NoteDocument::open(self.vault, self.id, self.key()).unwrap()
        }

        /// A fresh replica that has applied `updates` in order.
        fn replica(&self, updates: &[&EncryptedUpdate]) -> NoteDocument {
            let mut doc = self.open();
            for update in updates {
                doc.apply_update(update.as_bytes()).unwrap();
            }
            doc
        }

        /// Encrypts `plaintext` as an update of this note, bypassing the
        /// document.
        fn raw_update(&self, plaintext: &[u8]) -> Vec<u8> {
            self.key()
                .encrypt(self.vault, self.id, PayloadKind::CrdtUpdate, plaintext)
                .unwrap()
        }

        fn raw_snapshot(&self, plaintext: &[u8]) -> Vec<u8> {
            self.key()
                .encrypt(self.vault, self.id, PayloadKind::CrdtSnapshot, plaintext)
                .unwrap()
        }

        fn raw(&self, snapshot: bool, plaintext: &[u8]) -> Vec<u8> {
            if snapshot {
                self.raw_snapshot(plaintext)
            } else {
                self.raw_update(plaintext)
            }
        }

        fn plaintext(&self, update: &EncryptedUpdate) -> Vec<u8> {
            self.key()
                .decrypt(
                    self.vault,
                    self.id,
                    PayloadKind::CrdtUpdate,
                    update.as_bytes(),
                )
                .unwrap()
                .to_vec()
        }
    }

    /// Everything observable about a document's state.
    fn state(doc: &NoteDocument) -> (Option<Note>, StateVector, Vec<u8>) {
        let txn = doc.doc.transact();
        (
            doc.note(),
            txn.state_vector(),
            txn.encode_state_as_update_v1(&StateVector::default()),
        )
    }

    fn body(doc: &NoteDocument) -> String {
        doc.note().unwrap().body().to_owned()
    }

    fn assert_same(a: &NoteDocument, b: &NoteDocument) {
        assert_eq!(a.note(), b.note());
        assert_eq!(
            a.doc.transact().state_vector(),
            b.doc.transact().state_vector()
        );
    }

    /// A small deterministic PRNG (xorshift64*) for property tests.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }

        fn bytes(&mut self, len: usize) -> Vec<u8> {
            (0..len).map(|_| self.next() as u8).collect()
        }

        /// Text drawn from pieces that stress offsets: multi-byte, surrogate
        /// pairs, combining marks, ZWJ sequences, and CRLF.
        fn text(&mut self) -> String {
            const PIECES: &[&str] = &[
                "a",
                "b",
                "c",
                " ",
                "\n",
                "\r\n",
                "#tag",
                "é",
                "e\u{301}",
                "日本",
                "😀",
                "👩‍💻",
                "🇸🇪",
                "\u{200d}",
            ];
            let len = self.below(12);
            (0..len).map(|_| PIECES[self.below(PIECES.len())]).collect()
        }
    }

    // Document per note

    #[test]
    fn new_document_matches_the_note_it_was_created_from() {
        let f = Fixture::new();
        let (doc, _) = f.create("# Hello\nworld");
        let note = doc.note().unwrap();
        assert_eq!(note.id(), f.id);
        assert_eq!(note.body(), "# Hello\nworld");
        assert_eq!(note.created(), T1);
        assert_eq!(note.updated(), T1);
        assert!(!note.is_pinned() && !note.is_archived() && !note.is_trashed());
        assert_eq!(doc.id(), f.id);
        assert_eq!(doc.vault_id(), f.vault);
    }

    #[test]
    fn creation_produces_an_initial_update() {
        let f = Fixture::new();
        let (doc, update) = f.create("# Hello\nworld");
        let replica = f.replica(&[&update]);
        assert_eq!(replica.note(), doc.note());
    }

    #[test]
    fn debug_output_is_redacted() {
        let f = Fixture::new();
        let (doc, _) = f.create("secret marker text");
        let debug = format!("{doc:?}");
        assert!(debug.starts_with("NoteDocument"));
        assert!(debug.contains(&format!("{:?}", f.id)));
        assert!(!debug.contains("secret marker text"));
        assert!(!debug.contains("key"));
    }

    #[test]
    fn replicas_draw_different_32_bit_client_ids() {
        let f = Fixture::new();
        let (a, b) = (f.open(), f.open());
        assert_ne!(a.doc.client_id(), b.doc.client_id());
        assert!(a.doc.client_id().get() <= u64::from(u32::MAX));
    }

    // Current note state

    #[test]
    fn title_derived_from_the_merged_body() {
        let f = Fixture::new();
        let (mut a, u0) = f.create("## Plans ##");
        let mut b = f.replica(&[&u0]);
        let ua = a.set_body("## Plans ##\n- ship", T2).unwrap().unwrap();
        b.apply_update(ua.as_bytes()).unwrap();
        assert_eq!(body(&b), "## Plans ##\n- ship");
        assert_eq!(b.note().unwrap().title(), "Plans");
    }

    #[test]
    fn unreadable_metadata_falls_back_to_defaults() {
        let f = Fixture::new();
        let (mut doc, _) = f.create("x");
        doc.set_pinned(true).unwrap().unwrap();
        doc.set_archived(true).unwrap().unwrap();

        // A writer that has seen everything stores the wrong types.
        let mut writer = f.open();
        writer
            .load_snapshot(doc.snapshot().unwrap().as_bytes())
            .unwrap();
        let plaintext = {
            let mut txn = writer.doc.transact_mut();
            writer.meta.insert(&mut txn, META_PINNED, "yes");
            writer
                .meta
                .insert(&mut txn, META_ARCHIVED, Any::Number(Number::Int(1)));
            writer.meta.insert(&mut txn, META_UPDATED, "yesterday");
            writer.meta.insert(&mut txn, "unknown", 42);
            let mut plaintext = vec![FORMAT_V1];
            plaintext.extend(txn.encode_update_v1());
            plaintext
        };
        doc.apply_update(&f.raw_update(&plaintext)).unwrap();

        let note = doc.note().unwrap();
        assert!(!note.is_pinned());
        assert!(!note.is_archived());
        assert_eq!(note.updated(), note.created());
        assert_eq!(note.body(), "x");
    }

    #[test]
    fn timestamps_written_as_floats_are_read() {
        let f = Fixture::new();
        let (doc, _) = f.create("x");
        let set_updated = |value: f64| {
            let mut txn = doc.doc.transact_mut();
            doc.meta
                .insert(&mut txn, META_UPDATED, Any::Number(Number::Float(value)));
        };
        set_updated(T2.0 as f64);
        assert_eq!(doc.note().unwrap().updated(), T2);
        set_updated(1.5);
        assert_eq!(doc.note().unwrap().updated(), T1);
        set_updated(f64::NAN);
        assert_eq!(doc.note().unwrap().updated(), T1);
        set_updated(1e300);
        assert_eq!(doc.note().unwrap().updated(), T1);
    }

    #[test]
    fn document_missing_its_creation_update() {
        let f = Fixture::new();
        let (mut a, _) = f.create("draft");
        let later = a.set_body("final", T2).unwrap().unwrap();
        let b = f.replica(&[&later]);
        assert_eq!(b.note(), None);
        assert_eq!(f.open().note(), None);
    }

    // Local edits produce encrypted updates

    #[test]
    fn body_edit_yields_an_update() {
        let f = Fixture::new();
        let (mut a, u0) = f.create("Draft");
        let mut b = f.replica(&[&u0]);
        let update = a.set_body("Final draft", T2).unwrap().unwrap();
        b.apply_update(update.as_bytes()).unwrap();
        assert_eq!(body(&a), "Final draft");
        assert_eq!(body(&b), "Final draft");
    }

    #[test]
    fn flag_change_yields_an_update() {
        let f = Fixture::new();
        let (mut a, u0) = f.create("x");
        let mut b = f.replica(&[&u0]);

        let update = a.set_pinned(true).unwrap().unwrap();
        b.apply_update(update.as_bytes()).unwrap();
        assert!(b.note().unwrap().is_pinned());

        let update = a.set_archived(true).unwrap().unwrap();
        b.apply_update(update.as_bytes()).unwrap();
        assert!(b.note().unwrap().is_archived());

        let update = a.set_trashed(true).unwrap().unwrap();
        b.apply_update(update.as_bytes()).unwrap();
        assert!(b.note().unwrap().is_trashed());

        let update = a.set_pinned(false).unwrap().unwrap();
        b.apply_update(update.as_bytes()).unwrap();
        assert!(!b.note().unwrap().is_pinned());
        assert_eq!(a.note(), b.note());
    }

    #[test]
    fn no_op_change_yields_nothing() {
        let f = Fixture::new();
        let (mut doc, _) = f.create("same");
        let before = state(&doc);
        assert!(doc.set_body("same", T2).unwrap().is_none());
        assert!(doc.set_pinned(false).unwrap().is_none());
        assert!(doc.set_archived(false).unwrap().is_none());
        assert!(doc.set_trashed(false).unwrap().is_none());
        assert_eq!(state(&doc), before);

        doc.set_pinned(true).unwrap().unwrap();
        assert!(doc.set_pinned(true).unwrap().is_none());
    }

    // Timestamps follow the note model

    #[test]
    fn body_edit_bumps_updated_time() {
        let f = Fixture::new();
        let (mut doc, _) = f.create("a");
        doc.set_body("b", T2).unwrap().unwrap();
        let note = doc.note().unwrap();
        assert_eq!(note.updated(), T2);
        assert_eq!(note.created(), T1);
    }

    #[test]
    fn flag_change_leaves_updated_time_alone() {
        let f = Fixture::new();
        let (mut doc, _) = f.create("a");
        doc.set_archived(true).unwrap().unwrap();
        doc.set_pinned(true).unwrap().unwrap();
        doc.set_trashed(true).unwrap().unwrap();
        assert_eq!(doc.note().unwrap().updated(), T1);
    }

    #[test]
    fn set_body_round_trips_awkward_text() {
        let f = Fixture::new();
        let cases = [
            ("line\r\nline", "line\r\nnew\r\nline"),
            ("a\r\nb", "a\nb"),
            ("a😀b", "a😁b"),
            ("😀😀", "😀"),
            ("🇸🇪 flag", "🇳🇴 flag"),
            ("cafe\u{301}", "cafe\u{300}"),
            ("e\u{301}", "e"),
            ("", "👩‍💻 dev"),
            ("👩‍💻 dev", ""),
        ];
        for (from, to) in cases {
            let (mut doc, u0) = f.create(from);
            let mut replica = f.replica(&[&u0]);
            let update = doc.set_body(to, T2).unwrap().unwrap();
            assert_eq!(body(&doc), to, "{from:?} -> {to:?}");
            replica.apply_update(update.as_bytes()).unwrap();
            assert_eq!(body(&replica), to, "{from:?} -> {to:?}");
        }
    }

    #[test]
    fn set_body_always_produces_the_target_text() {
        let f = Fixture::new();
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let (mut doc, u0) = f.create("");
        let mut replica = f.replica(&[&u0]);
        for _ in 0..500 {
            let target = rng.text();
            if let Some(update) = doc.set_body(&target, T2).unwrap() {
                replica.apply_update(update.as_bytes()).unwrap();
            }
            assert_eq!(body(&doc), target);
            assert_eq!(body(&replica), target);
        }
    }

    // Bad input leaves the document unchanged

    #[test]
    fn update_from_another_note_is_rejected() {
        let (fa, fb) = (Fixture::for_note(1), Fixture::for_note(2));
        let (_, from_a) = fa.create("note a");
        let (mut b, _) = fb.create("note b");
        let before = state(&b);
        assert_eq!(
            b.apply_update(from_a.as_bytes()),
            Err(DocumentError::Crypto(CryptoError::Decrypt))
        );
        assert_eq!(state(&b), before);
    }

    #[test]
    fn tampered_update() {
        let f = Fixture::new();
        let (mut a, u0) = f.create("abc");
        let mut b = f.replica(&[&u0]);
        let mut update = a.set_body("abcd", T2).unwrap().unwrap().into_bytes();
        let last = update.len() - 1;
        update[last] ^= 1;
        let before = state(&b);
        assert_eq!(
            b.apply_update(&update),
            Err(DocumentError::Crypto(CryptoError::Decrypt))
        );
        assert_eq!(state(&b), before);
    }

    #[test]
    fn garbage_that_decrypts() {
        let f = Fixture::new();
        let (mut doc, _) = f.create("abc");
        let before = state(&doc);
        for garbage in [
            &[][..],
            &[FORMAT_V1][..],
            &[FORMAT_V1, 0xff, 0xff, 0xff],
            &[FORMAT_V1, 5],
        ] {
            for snapshot in [false, true] {
                let ciphertext = f.raw(snapshot, garbage);
                let result = if snapshot {
                    doc.load_snapshot(&ciphertext)
                } else {
                    doc.apply_update(&ciphertext)
                };
                assert_eq!(result, Err(DocumentError::Malformed), "{garbage:?}");
                assert_eq!(state(&doc), before);
            }
        }
    }

    #[test]
    fn unknown_document_format() {
        let f = Fixture::new();
        let (mut doc, u0) = f.create("abc");
        let mut plaintext = f.plaintext(&u0);
        plaintext[0] = 2;
        let before = state(&doc);
        assert_eq!(
            doc.apply_update(&f.raw_update(&plaintext)),
            Err(DocumentError::UnsupportedFormat(2))
        );
        assert_eq!(
            doc.load_snapshot(&f.raw_snapshot(&plaintext)),
            Err(DocumentError::UnsupportedFormat(2))
        );
        assert_eq!(state(&doc), before);
    }

    fn hex(text: &str) -> Vec<u8> {
        let text: String = text.split_whitespace().collect();
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Asserts that `plaintext` is rejected as malformed, both as an update
    /// and as a snapshot, and leaves `doc` unchanged.
    fn assert_rejected(f: &Fixture, doc: &mut NoteDocument, plaintext: &[u8], why: &str) {
        let before = state(doc);
        assert_eq!(
            doc.apply_update(&f.raw_update(plaintext)),
            Err(DocumentError::Malformed),
            "{why}"
        );
        assert_eq!(
            doc.load_snapshot(&f.raw_snapshot(plaintext)),
            Err(DocumentError::Malformed),
            "{why}"
        );
        assert_eq!(state(doc), before, "{why}");
    }

    #[test]
    fn update_that_would_partly_apply_is_rejected() {
        // Client 99 inserts "hi" into `body`, then names that string item
        // as the parent of a second block.
        let f = Fixture::new();
        let (mut doc, _) = f.create("abc");
        let plaintext = hex("01 01 02 63 00 04 01 04 626f6479 02 6869 04 00 63 00 01 78 00");
        assert_rejected(&f, &mut doc, &plaintext, "partial apply");
        assert_eq!(body(&doc), "abc");
    }

    #[test]
    fn held_bad_update_cannot_poison_a_valid_one() {
        let f = Fixture::new();
        let (mut doc, _) = f.create("abc");
        // Client 77's block names missing item (66, 0) as its parent.
        let held = hex("01 01 01 4d 00 04 00 42 00 01 78 00");
        assert_rejected(&f, &mut doc, &held, "explicit-ID parent");
        // Client 66 inserts "z" into `body`.
        let valid = hex("01 01 01 42 00 04 01 04 626f6479 01 7a 00");
        doc.apply_update(&f.raw_update(&valid)).unwrap();
        assert_eq!(body(&doc), "zabc");
    }

    #[test]
    fn clock_overflow_is_rejected() {
        let f = Fixture::new();
        let (mut doc, _) = f.create("abc");
        let cases = [
            ("01 01 01 00 0a ffffffff0f 00", "skip of u32::MAX"),
            ("01 01 01 00 00 ffffffff0f 00", "gc of u32::MAX"),
            ("01 00 01 01 01 feffffff0f 05", "delete range past u32::MAX"),
            (
                "01 01 01 feffffff0f 04 01 04 626f6479 03 616263 00",
                "string at u32::MAX - 1",
            ),
        ];
        for (bytes, why) in cases {
            assert_rejected(&f, &mut doc, &hex(bytes), why);
        }
    }

    /// Applies each plaintext in turn. A rejected one must leave the state
    /// unchanged; afterwards the document must still snapshot, reload to the
    /// same body, and take a local edit.
    fn survives(f: &Fixture, mut doc: NoteDocument, plaintexts: &[&str]) {
        for (i, plaintext) in plaintexts.iter().enumerate() {
            let before = state(&doc);
            if doc.apply_update(&f.raw_update(&hex(plaintext))).is_err() {
                assert_eq!(state(&doc), before, "update {i}");
            }
        }
        let mut reloaded = f.open();
        reloaded
            .load_snapshot(doc.snapshot().unwrap().as_bytes())
            .unwrap();
        assert_eq!(
            reloaded.note().map(|n| n.body().to_owned()),
            doc.note().map(|n| n.body().to_owned())
        );
        if doc.note().is_some() {
            doc.set_body("after", T2).unwrap();
            assert_eq!(body(&doc), "after");
        }
    }

    #[test]
    fn second_review_crash_payloads() {
        let f = Fixture::new();
        // Zero-length string from a client that already has a block.
        survives(
            &f,
            f.open(),
            &[
                "01 01 01 05 00 04 01 04 626f6479 01 78 00",
                "01 01 01 05 00 04 01 04 626f6479 00 00",
            ],
        );
        // Out-of-order GC blocks overlapping an integrated skip.
        survives(
            &f,
            f.open(),
            &[
                "01 01 03 01 01 00 02 00 03 00 02 02 02 01 09 01 01 01 08 03",
                "01 02 02 01 00 00 02 00 04 04 02 01 00 02 00 04 00 02 00 03 02 01 01 0b 04 01 01 0a 00",
            ],
        );
        // Clock offsets at 2^31.
        survives(&f, f.open(), &["01 01 01 05 8080808008 00 00 00"]);
        survives(
            &f,
            f.open(),
            &[
                "01 01 01 05 ffffffff07 04 01 04 626f6479 01 78 00",
                "01 01 01 05 00 04 01 04 626f6479 01 79 00",
            ],
        );
        // One client in two sections with clocks going backwards, over
        // content that is already integrated.
        let creator = || {
            NoteDocument::create_with_client_id(f.vault, f.id, f.key(), "hello world", T1, 1000)
                .unwrap()
                .0
        };
        survives(
            &f,
            creator(),
            &["01 02 01 e807 0f 04 01 04 626f6479 01 61 01 e807 02 00 03 00"],
        );
        survives(
            &f,
            f.open(),
            &["01 02 01 05 00 04 01 04 626f6479 01 42 01 05 00 04 01 04 626f6479 01 41 00"],
        );
    }

    #[test]
    fn allocation_bomb_is_rejected() {
        let f = Fixture::new();
        let (mut doc, _) = f.create("abc");
        // Declares u32::MAX clients in six bytes.
        assert_rejected(&f, &mut doc, &hex("01 ffffffff0f"), "client count");
        assert_rejected(
            &f,
            &mut doc,
            &hex("01 01 01 42 00 08 01 04 6d657461 01 76 ffffffff0f 00"),
            "any map count",
        );
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let f = Fixture::new();
        let (mut doc, _) = f.create("abc");
        let plaintext = hex("01 01 01 42 00 04 01 04 626f6479 01 7a 00 deadbeef");
        assert_rejected(&f, &mut doc, &plaintext, "trailing bytes");
    }

    #[test]
    fn out_of_schema_content_is_rejected() {
        let f = Fixture::new();
        let (mut doc, _) = f.create("abc");
        let cases = [
            ("01 01 01 42 00 04 01 03 666f6f 01 7a 00", "foreign root"),
            ("01 01 01 42 00 05 01 04 626f6479 01 7b 00", "embed"),
            ("01 01 01 42 00 07 01 04 626f6479 00 00", "nested type"),
            ("01 01 01 42 00 03 01 04 626f6479 01 00 00", "binary"),
            ("01 01 01 42 00 09 01 04 6d657461 00", "subdocument"),
        ];
        for (bytes, why) in cases {
            assert_rejected(&f, &mut doc, &hex(bytes), why);
        }
    }

    #[test]
    fn oversized_input_is_rejected() {
        let f = Fixture::new();
        let (mut doc, _) = f.create("abc");
        let mut plaintext = vec![0; MAX_PLAINTEXT_LEN + 1];
        plaintext[0] = FORMAT_V1;
        assert_rejected(&f, &mut doc, &plaintext, "one byte over the cap");
        let too_long = vec![0; ENVELOPE_OVERHEAD + 2 + MAX_PLAINTEXT_LEN];
        assert_eq!(doc.apply_update(&too_long), Err(DocumentError::Malformed));
    }

    #[test]
    fn set_body_with_a_value_in_the_body() {
        // Client 99 puts a value at the start of `body`: invisible text that
        // insertion counts as an offset but removal skips.
        let f = Fixture::new();
        let (mut doc, u0) = f.create("hello");
        let value = f.raw_update(&hex("01 01 01 63 00 08 01 04 626f6479 01 7d 05 00"));
        doc.apply_update(&value).unwrap();
        assert_eq!(body(&doc), "hello");

        let mut replica = f.replica(&[&u0]);
        replica.apply_update(&value).unwrap();
        let update = doc.set_body("hello world", T2).unwrap().unwrap();
        assert_eq!(body(&doc), "hello world");
        replica.apply_update(update.as_bytes()).unwrap();
        assert_eq!(body(&replica), "hello world");
        assert_same(&doc, &replica);
    }

    #[test]
    fn client_id_collision_rotates_the_id() {
        let f = Fixture::new();
        let (mut a, u0) =
            NoteDocument::create_with_client_id(f.vault, f.id, f.key(), "x", T1, 7).unwrap();
        let mut b = NoteDocument::with_client_id(f.vault, f.id, f.key(), 7);
        b.apply_update(u0.as_bytes()).unwrap();
        assert_ne!(b.doc.client_id().get(), 7);

        let ub = b.set_body("x from b", T2).unwrap().unwrap();
        let ua = a.set_pinned(true).unwrap().unwrap();
        a.apply_update(ub.as_bytes()).unwrap();
        b.apply_update(ua.as_bytes()).unwrap();
        assert_eq!(a.doc.client_id().get(), 7);
        assert_same(&a, &b);
        let note = b.note().unwrap();
        assert_eq!(note.body(), "x from b");
        assert!(note.is_pinned());
    }

    #[test]
    fn own_updates_coming_back_keep_the_id() {
        let f = Fixture::new();
        let (mut doc, u0) = f.create("x");
        let id = doc.doc.client_id();
        let u1 = doc.set_body("xy", T2).unwrap().unwrap();
        doc.apply_update(u0.as_bytes()).unwrap();
        doc.apply_update(u1.as_bytes()).unwrap();
        doc.load_snapshot(doc.snapshot().unwrap().as_bytes())
            .unwrap();
        assert_eq!(doc.doc.client_id(), id);
    }

    #[test]
    fn emitted_payloads_pass_the_check() {
        let f = Fixture::new();
        let (mut a, u0) = f.create("one 😀");
        let u1 = a.set_body("one two 😀", T2).unwrap().unwrap();
        let u2 = a.set_body("two 😀 three", T2).unwrap().unwrap();
        let u3 = a.set_trashed(true).unwrap().unwrap();
        for update in [&u0, &u1, &u2, &u3] {
            assert_eq!(super::check_update(&f.plaintext(update)[1..]), Ok(()));
        }
        // Snapshots, including one carrying a held update and deletions.
        let holder = f.replica(&[&u0, &u2]);
        for doc in [&a, &holder] {
            let snapshot = doc.snapshot().unwrap();
            let plaintext = f
                .key()
                .decrypt(
                    f.vault,
                    f.id,
                    PayloadKind::CrdtSnapshot,
                    snapshot.as_bytes(),
                )
                .unwrap();
            assert_eq!(super::check_update(&plaintext[1..]), Ok(()));
        }
    }

    #[test]
    fn bad_input_never_panics_or_changes_state() {
        let f = Fixture::new();
        let (mut source, u0) = f.create("one\ntwo");
        let mut valid = vec![f.plaintext(&u0)];
        for (i, text) in ["one\nTWO", "zero\none\nTWO", "zero\nTWO 😀"]
            .into_iter()
            .enumerate()
        {
            let update = source
                .set_body(text, Timestamp(T2.0 + i as i64))
                .unwrap()
                .unwrap();
            valid.push(f.plaintext(&update));
        }
        let pin = source.set_pinned(true).unwrap().unwrap();
        valid.push(f.plaintext(&pin));
        let pin = pin.into_bytes();
        let snapshot = source.snapshot().unwrap().into_bytes();
        let source_client = source.doc.client_id().get() as u32;

        let mut rng = Rng(0x1234_5678_9abc_def1);
        let mut target = f.replica(&[]);
        target.apply_update(&f.raw_update(&valid[0])).unwrap();
        let mut accepted_structured = 0;
        for round in 0..4000 {
            let before = state(&target);
            let as_snapshot = round % 2 == 1;
            let real = if as_snapshot { &snapshot } else { &pin };
            let ciphertext = match round % 6 {
                // Byte flips and truncations of a real ciphertext of the
                // right kind.
                0 => {
                    let mut bytes = real.clone();
                    let i = rng.below(bytes.len());
                    bytes[i] ^= 1 << rng.below(8);
                    bytes
                }
                1 => real[..rng.below(real.len())].to_vec(),
                // Well-formed updates with extreme clocks, lengths, counts,
                // content, and parents.
                5 => {
                    let mut plaintext = vec![FORMAT_V1];
                    plaintext.extend(structured_update(&mut rng, source_client));
                    f.raw(as_snapshot, &plaintext)
                }
                // Random plaintext behind a valid format byte.
                2 => {
                    let mut plaintext = vec![FORMAT_V1];
                    let len = rng.below(64);
                    plaintext.extend(rng.bytes(len));
                    f.raw(as_snapshot, &plaintext)
                }
                // Mutations and truncations of real update plaintexts.
                _ => {
                    let mut plaintext = valid[rng.below(valid.len())].clone();
                    if rng.below(2) == 0 {
                        for _ in 0..1 + rng.below(3) {
                            let i = 1 + rng.below(plaintext.len() - 1);
                            plaintext[i] = rng.next() as u8;
                        }
                    } else {
                        plaintext.truncate(1 + rng.below(plaintext.len() - 1));
                    }
                    f.raw(as_snapshot, &plaintext)
                }
            };
            let result = if as_snapshot {
                target.load_snapshot(&ciphertext)
            } else {
                target.apply_update(&ciphertext)
            };
            if result.is_err() {
                assert_eq!(state(&target), before, "round {round}: {result:?}");
            } else {
                accepted_structured += usize::from(round % 6 == 5);
                if target.note().is_some() {
                    // Whatever got in, local edits still produce exactly
                    // the text asked for.
                    let probe = format!("probe {round}");
                    target.set_body(&probe, T2).unwrap();
                    assert_eq!(body(&target), probe);
                }
            }
        }
        // The generator must reach integration, not just the check.
        assert!(accepted_structured > 20, "{accepted_structured}");
    }

    fn write_var(out: &mut Vec<u8>, mut n: u64) {
        while n >= 0x80 {
            out.push(n as u8 | 0x80);
            n >>= 7;
        }
        out.push(n as u8);
    }

    fn write_string(out: &mut Vec<u8>, s: &str) {
        write_var(out, s.len() as u64);
        out.extend_from_slice(s.as_bytes());
    }

    /// A structurally plausible v1 update, biased towards edge cases.
    fn structured_update(rng: &mut Rng, known_client: u32) -> Vec<u8> {
        let client = |rng: &mut Rng| match rng.below(3) {
            0 => u64::from(known_client),
            1 => rng.below(4) as u64,
            _ => rng.next() & 0xffff_ffff,
        };
        let clock = |rng: &mut Rng| match rng.below(4) {
            0 => rng.below(8) as u64,
            1 => u64::from(u32::MAX) - rng.below(4) as u64,
            2 => (1 << 31) - rng.below(4) as u64,
            _ => rng.next() & 0xffff_ffff,
        };
        let mut out = Vec::new();
        let clients = rng.below(3);
        write_var(&mut out, clients as u64);
        for _ in 0..clients {
            let blocks = 1 + rng.below(3);
            write_var(&mut out, blocks as u64);
            write_var(&mut out, client(rng));
            write_var(&mut out, clock(rng));
            for _ in 0..blocks {
                match rng.below(6) {
                    0 => {
                        out.push(10);
                        write_var(&mut out, clock(rng));
                    }
                    1 => {
                        out.push(0);
                        write_var(&mut out, clock(rng));
                    }
                    _ => {
                        let flags = [0, 0x80, 0x40, 0xc0, 0x20][rng.below(5)];
                        let content = [1, 4, 4, 8, 8, 5, 7, rng.below(16) as u8][rng.below(8)];
                        out.push(flags | content);
                        for flag in [0x80, 0x40] {
                            if flags & flag != 0 {
                                write_var(&mut out, client(rng));
                                write_var(&mut out, clock(rng));
                            }
                        }
                        if flags & 0xc0 == 0 {
                            if rng.below(4) == 0 {
                                out.push(0);
                                write_var(&mut out, client(rng));
                                write_var(&mut out, clock(rng));
                            } else {
                                out.push(1);
                                write_string(&mut out, ["body", "meta", "x"][rng.below(3)]);
                            }
                            if flags & 0x20 != 0 {
                                write_string(
                                    &mut out,
                                    ["pinned", "updated", "created"][rng.below(3)],
                                );
                            }
                        }
                        match content {
                            1 => write_var(&mut out, clock(rng)),
                            4 => write_string(&mut out, &rng.text()),
                            8 => {
                                out.push(1);
                                out.extend(
                                    [[0x78].as_slice(), &[0x7d, 0x05], &[0x77, 0x01, 0x61]]
                                        [rng.below(3)],
                                );
                            }
                            _ => {
                                let len = rng.below(4);
                                out.extend(rng.bytes(len));
                            }
                        }
                    }
                }
            }
        }
        let deletes = rng.below(3);
        write_var(&mut out, deletes as u64);
        for _ in 0..deletes {
            write_var(&mut out, client(rng));
            write_var(&mut out, 1);
            write_var(&mut out, clock(rng));
            write_var(&mut out, clock(rng));
        }
        out
    }

    // Merging and convergence

    #[test]
    fn concurrent_edits_to_different_lines() {
        let f = Fixture::new();
        let (mut a, u0) = f.create("one\ntwo\nthree");
        let mut b = f.replica(&[&u0]);
        let ua = a.set_body("ONE\ntwo\nthree", T2).unwrap().unwrap();
        let ub = b.set_body("one\ntwo\nTHREE", T2).unwrap().unwrap();
        a.apply_update(ub.as_bytes()).unwrap();
        b.apply_update(ua.as_bytes()).unwrap();
        assert_eq!(body(&a), "ONE\ntwo\nTHREE");
        assert_same(&a, &b);
    }

    #[test]
    fn setting_the_body_keeps_concurrent_edits_between_changed_spans() {
        let f = Fixture::new();
        let (mut a, u0) = f.create("#a\nmiddle\n#a");
        let mut b = f.replica(&[&u0]);
        let ua = a.set_body("#b\nmiddle\n#b", T2).unwrap().unwrap();
        let ub = b.set_body("#a\ncenter\n#a", T2).unwrap().unwrap();
        a.apply_update(ub.as_bytes()).unwrap();
        b.apply_update(ua.as_bytes()).unwrap();
        assert_eq!(body(&a), "#b\ncenter\n#b");
        assert_same(&a, &b);
    }

    #[test]
    fn concurrent_inserts_at_the_same_place() {
        let f = Fixture::new();
        let (mut a, u0) = f.create("ab");
        let mut b = f.replica(&[&u0]);
        let ua = a.set_body("aXXb", T2).unwrap().unwrap();
        let ub = b.set_body("aYYb", T2).unwrap().unwrap();
        a.apply_update(ub.as_bytes()).unwrap();
        b.apply_update(ua.as_bytes()).unwrap();
        let merged = body(&a);
        assert!(merged == "aXXYYb" || merged == "aYYXXb", "{merged:?}");
        assert_same(&a, &b);
    }

    #[test]
    fn any_order() {
        let f = Fixture::new();
        let (mut a, u0) = f.create("one\ntwo\nthree");
        let mut b = f.replica(&[&u0]);
        let mut c = f.replica(&[&u0]);
        let ua = a.set_body("ONE\ntwo\nthree", T2).unwrap().unwrap();
        let ub1 = b.set_body("one\ntwo\nthree\nfour", T2).unwrap().unwrap();
        let ub2 = b.set_pinned(true).unwrap().unwrap();
        let uc1 = c.set_body("one\nTWO\nthree", T2).unwrap().unwrap();
        let uc2 = c.set_archived(true).unwrap().unwrap();

        let forward = f.replica(&[&u0, &ua, &ub1, &ub2, &uc1, &uc2]);
        let backward = f.replica(&[&uc2, &uc1, &ub2, &ub1, &ua, &u0]);
        let shuffled = f.replica(&[&ub2, &uc1, &u0, &uc2, &ua, &ub1]);
        assert_same(&forward, &backward);
        assert_same(&forward, &shuffled);
        let note = forward.note().unwrap();
        assert_eq!(note.body(), "ONE\nTWO\nthree\nfour");
        assert!(note.is_pinned() && note.is_archived());
    }

    #[test]
    fn duplicates_are_harmless() {
        let f = Fixture::new();
        let (mut a, u0) = f.create("x");
        let u1 = a.set_body("xy", T2).unwrap().unwrap();
        let mut b = f.replica(&[&u0, &u1]);
        let before = state(&b);
        b.apply_update(u1.as_bytes()).unwrap();
        b.apply_update(u0.as_bytes()).unwrap();
        assert_eq!(state(&b), before);
        assert_eq!(body(&b), "xy");
    }

    #[test]
    fn concurrent_flag_changes() {
        let f = Fixture::new();
        let (mut a, u0) = f.create("x");
        let mut b = f.replica(&[&u0]);
        let pin = a.set_pinned(true).unwrap().unwrap();
        // `b` writes `false` concurrently: pin then unpin locally.
        let b_pin = b.set_pinned(true).unwrap().unwrap();
        let b_unpin = b.set_pinned(false).unwrap().unwrap();
        a.apply_update(b_pin.as_bytes()).unwrap();
        a.apply_update(b_unpin.as_bytes()).unwrap();
        b.apply_update(pin.as_bytes()).unwrap();
        assert_eq!(a.note().unwrap().is_pinned(), b.note().unwrap().is_pinned());
        assert_same(&a, &b);
    }

    #[test]
    fn held_update_integrates_when_a_skip_fills() {
        // u2's text insert depends only on the creation update, so Yrs
        // integrates it early and leaves a skip for u1. u2's `updated` entry
        // depends on u1's and is held. u1 then fills the skip without moving
        // C's clock, which used to leave the held entry stuck.
        let f = Fixture::new();
        let (_, u0) = f.create("start");
        let mut c = f.replica(&[&u0]);
        let u1 = c.set_body("Xstart", T2).unwrap().unwrap();
        let u2 = c.set_body("XstartY", Timestamp(T2.0 + 5)).unwrap().unwrap();
        let u3 = c.set_trashed(true).unwrap().unwrap();
        assert_same(&c, &f.replica(&[&u0, &u2, &u1, &u3]));
        let late = f.replica(&[&u0, &u3, &u2, &u1]);
        assert_same(&c, &late);
        assert_eq!(late.note().unwrap().updated(), Timestamp(T2.0 + 5));
    }

    #[test]
    fn shuffled_delivery_converges() {
        let f = Fixture::new();
        let mut rng = Rng(0x5eed_0fc0_ffee_4242);
        for _ in 0..40 {
            let (_, u0) = f.create("one two three");
            let mut replicas = [f.replica(&[&u0]), f.replica(&[&u0]), f.replica(&[&u0])];
            let mut updates = Vec::new();
            for step in 0..8 {
                let i = rng.below(3);
                let update = if rng.below(4) == 0 {
                    replicas[i].set_pinned(rng.below(2) == 0).unwrap()
                } else {
                    let text = format!("{} {}", body(&replicas[i]), rng.text());
                    replicas[i].set_body(&text, Timestamp(T2.0 + step)).unwrap()
                };
                if let Some(update) = update {
                    // Sometimes share it right away, as live sync would.
                    if rng.below(2) == 0 {
                        let j = rng.below(3);
                        if j != i {
                            replicas[j].apply_update(update.as_bytes()).unwrap();
                        }
                    }
                    updates.push(update);
                }
            }
            let in_order: Vec<_> = std::iter::once(&u0).chain(&updates).collect();
            let reference = f.replica(&in_order);
            let mut shuffled = in_order.clone();
            for i in (1..shuffled.len()).rev() {
                shuffled.swap(i, rng.below(i + 1));
            }
            assert_same(&reference, &f.replica(&shuffled));
            for replica in &mut replicas {
                for update in &updates {
                    replica.apply_update(update.as_bytes()).unwrap();
                }
                assert_same(&reference, replica);
            }
        }
    }

    #[test]
    fn out_of_order_delivery() {
        let f = Fixture::new();
        let (mut a, u0) = f.create("one");
        let u1 = a.set_body("one two", T2).unwrap().unwrap();
        let u2 = a
            .set_body("one two three", Timestamp(T2.0 + 1))
            .unwrap()
            .unwrap();
        let mut b = f.replica(&[&u0, &u2]);
        assert_eq!(body(&b), "one");
        b.apply_update(u1.as_bytes()).unwrap();
        assert_eq!(body(&b), "one two three");
        assert_same(&a, &b);
    }

    // Snapshots

    #[test]
    fn snapshot_alone() {
        let f = Fixture::new();
        let (mut a, _) = f.create("one");
        a.set_body("one two", T2).unwrap().unwrap();
        a.set_trashed(true).unwrap().unwrap();
        let mut fresh = f.open();
        fresh
            .load_snapshot(a.snapshot().unwrap().as_bytes())
            .unwrap();
        assert_same(&a, &fresh);
    }

    #[test]
    fn full_update_reproduces_the_state() {
        let f = Fixture::new();
        let (mut a, _) = f.create("one");
        a.set_body("one two", T2).unwrap().unwrap();
        a.set_pinned(true).unwrap().unwrap();
        let full = a.full_update().unwrap();
        let mut fresh = f.open();
        fresh.apply_update(full.as_bytes()).unwrap();
        assert_same(&a, &fresh);
        // Encrypted as an update, so it doesn't load as a snapshot.
        assert!(matches!(
            f.open().load_snapshot(full.as_bytes()),
            Err(DocumentError::Crypto(_))
        ));
    }

    #[test]
    fn snapshot_plus_later_updates() {
        let f = Fixture::new();
        let (mut a, _) = f.create("one");
        let snapshot = a.snapshot().unwrap();
        let u1 = a.set_body("one two", T2).unwrap().unwrap();
        let u2 = a.set_pinned(true).unwrap().unwrap();
        let mut fresh = f.open();
        fresh.load_snapshot(snapshot.as_bytes()).unwrap();
        fresh.apply_update(u1.as_bytes()).unwrap();
        fresh.apply_update(u2.as_bytes()).unwrap();
        assert_same(&a, &fresh);
    }

    #[test]
    fn overlapping_updates() {
        let f = Fixture::new();
        let (mut a, u0) = f.create("one");
        let u1 = a.set_body("one two", T2).unwrap().unwrap();
        let snapshot = a.snapshot().unwrap();
        let mut fresh = f.open();
        fresh.load_snapshot(snapshot.as_bytes()).unwrap();
        let loaded = state(&fresh);
        fresh.apply_update(u0.as_bytes()).unwrap();
        fresh.apply_update(u1.as_bytes()).unwrap();
        assert_eq!(state(&fresh), loaded);
        assert_same(&a, &fresh);
    }

    #[test]
    fn snapshot_keeps_held_updates() {
        let f = Fixture::new();
        let (mut a, u0) = f.create("one");
        let u1 = a.set_body("one two", T2).unwrap().unwrap();
        let u2 = a
            .set_body("one two three", Timestamp(T2.0 + 1))
            .unwrap()
            .unwrap();
        let holder = f.replica(&[&u0, &u2]);
        let mut fresh = f.open();
        fresh
            .load_snapshot(holder.snapshot().unwrap().as_bytes())
            .unwrap();
        fresh.apply_update(u1.as_bytes()).unwrap();
        assert_eq!(body(&fresh), "one two three");
        assert_same(&a, &fresh);
    }

    #[test]
    fn update_presented_as_a_snapshot_is_rejected() {
        let f = Fixture::new();
        let (doc, update) = f.create("x");
        let mut fresh = f.open();
        assert_eq!(
            fresh.load_snapshot(update.as_bytes()),
            Err(DocumentError::Crypto(CryptoError::Decrypt))
        );
        assert_eq!(
            fresh.apply_update(doc.snapshot().unwrap().as_bytes()),
            Err(DocumentError::Crypto(CryptoError::Decrypt))
        );
        assert_eq!(fresh.note(), None);
    }

    #[test]
    fn ciphertext_hides_the_body() {
        let f = Fixture::new();
        let marker = b"ZEBRA-MARKER-42";
        let contains = |bytes: &[u8]| bytes.windows(marker.len()).any(|w| w == marker);
        let (mut doc, created) = f.create("start");
        let edit = doc.set_body("start ZEBRA-MARKER-42", T2).unwrap().unwrap();
        let snapshot = doc.snapshot().unwrap();
        // The marker is in the plaintext, so its absence below means
        // something.
        assert!(contains(&f.plaintext(&edit)));
        for ciphertext in [created.as_bytes(), edit.as_bytes(), snapshot.as_bytes()] {
            assert!(!contains(ciphertext));
        }
    }

    // Versioned document format

    /// Format-1 plaintext of the initial update of a fixed document. If a
    /// Yrs upgrade changes these bytes, the stored format has changed.
    const CREATE_VECTOR: &str = concat!(
        "01",       // format 1
        "01060100", // 1 client: 6 structs, client 1, clock 0
        "0401",
        "04626f6479",
        "0d232048656c6c6f0a776f726c64", // body: "# Hello\nworld"
        "2801",
        "046d657461",
        "0763726561746564",
        "017b4278bcfe56800000", // meta.created
        "2801",
        "046d657461",
        "0775706461746564",
        "017b4278bcfe56800000", // meta.updated
        "2801",
        "046d657461",
        "0670696e6e6564",
        "0179", // meta.pinned = false
        "2801",
        "046d657461",
        "086172636869766564",
        "0179", // meta.archived = false
        "2801",
        "046d657461",
        "0774726173686564",
        "0179", // meta.trashed = false
        "00",   // empty delete set
    );

    #[test]
    fn fixed_output_for_fixed_input() {
        let f = Fixture::with_ids(
            VaultId::from_parts(1_700_000_000_000, 0x0102_0304),
            NoteId::from_parts(1_700_000_000_000, 0x0506_0708),
        );
        let (_, update) =
            NoteDocument::create_with_client_id(f.vault, f.id, f.key(), "# Hello\nworld", T1, 1)
                .unwrap();
        let hex: String = f
            .plaintext(&update)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(hex, CREATE_VECTOR);
    }
}
