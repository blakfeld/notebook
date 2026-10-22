//! End-to-end flows through the public API only, as a client would use it.

use notebook_core::crypto::{
    AuthSecret, CryptoError, KdfVersion, KeyBundle, NoteKey, PayloadKind, RecoveryKey, Salt,
    VaultId, VaultKey, derive_auth_secret,
};
use notebook_core::note::{Note, NoteId, Timestamp};
use notebook_core::storage::{
    EntryKind, LocalUpdate, LogEntry, LogPos, MemoryStorage, OutboxEntry, RemoteBatch, RemoteEntry,
    ServerSeq, Storage, StorageError, StoredNote, UpdateId,
};
use notebook_core::tags::{TagPath, TagTree, View, rename, tags_of};

fn id(n: u128) -> NoteId {
    NoteId::from_parts(1_000, n)
}

/// Applies rename results the way a client would.
fn apply(notes: &mut [Note], changes: Vec<(NoteId, String)>, now: Timestamp) {
    for (id, body) in changes {
        notes
            .iter_mut()
            .find(|n| n.id() == id)
            .unwrap()
            .set_body(body, now);
    }
}

#[test]
fn version_is_exposed() {
    assert_eq!(notebook_core::version(), env!("CARGO_PKG_VERSION"));
}

#[test]
fn vault_lifecycle() {
    // Create a vault on one device.
    let vault: VaultId = "01ARZ3NDEKTSV4RRFFQ69G5FAV".parse().unwrap();
    let vault_key = VaultKey::generate().unwrap();
    let auth = AuthSecret::from_bytes(&[0x42; 64]).unwrap();
    let recovery = RecoveryKey::generate().unwrap();
    let written_down = recovery.to_display().to_string();
    let bundle_bytes = KeyBundle::create(vault, &vault_key, &auth, &recovery)
        .unwrap()
        .to_bytes();

    // Encrypt a note and its key for the server.
    let note = Note::new(id(1), "# Secret plans\n#work", Timestamp(1));
    let note_key = NoteKey::generate().unwrap();
    let wrapped_key = vault_key
        .wrap_note_key(vault, note.id(), &note_key)
        .unwrap();
    let payload = note_key
        .encrypt(
            vault,
            note.id(),
            PayloadKind::NotePayload,
            note.body().as_bytes(),
        )
        .unwrap();
    drop((vault_key, note_key));

    // A second device signs in with the auth secret.
    let bundle = KeyBundle::from_bytes(&bundle_bytes).unwrap();
    assert_eq!(bundle.vault_id(), vault);
    let vault_key = bundle.unlock_with_auth(&auth).unwrap();
    let note_key = vault_key
        .unwrap_note_key(vault, note.id(), &wrapped_key)
        .unwrap();
    let body = note_key
        .decrypt(vault, note.id(), PayloadKind::NotePayload, &payload)
        .unwrap();
    assert_eq!(&*body, note.body().as_bytes());

    // The user forgets the passphrase and recovers with the written-down key.
    let mut bundle = KeyBundle::from_bytes(&bundle_bytes).unwrap();
    let recovery = RecoveryKey::parse(&written_down.to_lowercase()).unwrap();
    let salt = Salt::generate().unwrap();
    let new_auth = derive_auth_secret(b"new passphrase", &salt, KdfVersion::CURRENT).unwrap();
    bundle.recover_auth(&recovery, &new_auth).unwrap();

    // Old ciphertexts still open under the new secret; the old secret fails.
    let bundle = KeyBundle::from_bytes(&bundle.to_bytes()).unwrap();
    let vault_key = bundle.unlock_with_auth(&new_auth).unwrap();
    let note_key = vault_key
        .unwrap_note_key(vault, note.id(), &wrapped_key)
        .unwrap();
    assert!(
        note_key
            .decrypt(vault, note.id(), PayloadKind::NotePayload, &payload)
            .is_ok()
    );
    assert!(matches!(
        bundle.unlock_with_auth(&auth),
        Err(CryptoError::Decrypt)
    ));
}

#[test]
fn server_cannot_move_ciphertexts_between_notes() {
    let vault = VaultId::from_parts(1_000, 1);
    let vault_key = VaultKey::generate().unwrap();
    let (a, b) = (NoteKey::generate().unwrap(), NoteKey::generate().unwrap());
    let wrapped_a = vault_key.wrap_note_key(vault, id(1), &a).unwrap();
    let update_a = a
        .encrypt(vault, id(1), PayloadKind::CrdtUpdate, b"edit")
        .unwrap();

    assert!(vault_key.unwrap_note_key(vault, id(2), &wrapped_a).is_err());
    assert_eq!(
        b.decrypt(vault, id(2), PayloadKind::CrdtUpdate, &update_a),
        Err(CryptoError::Decrypt)
    );
    assert_eq!(
        a.decrypt(vault, id(2), PayloadKind::CrdtUpdate, &update_a),
        Err(CryptoError::Decrypt)
    );
}

#[test]
fn organizing_notes_with_tags() {
    let mut notes = vec![
        Note::new(id(1), "# Standup\n#work/acme notes", Timestamp(1)),
        Note::new(id(2), "Groceries #home", Timestamp(2)),
        Note::new(id(3), "Loose thought", Timestamp(3)),
        Note::new(id(4), "Old project #work/beta", Timestamp(4)),
    ];
    notes[1].set_pinned(true);
    notes[3].set_archived(true);

    assert_eq!(notes[0].title(), "Standup");
    assert_eq!(
        tags_of(notes[0].body()),
        [TagPath::parse("work/acme").unwrap()]
    );

    let tree = TagTree::build(&notes);
    let roots: Vec<&str> = tree.roots().iter().map(|n| n.name.as_str()).collect();
    assert_eq!(roots, ["home", "work"]);
    assert!(
        tree.find(&TagPath::parse("work/beta").unwrap()).is_none(),
        "archived notes don't contribute"
    );

    assert_eq!(View::Notes.select(&notes), [id(3), id(2), id(1)]);
    assert_eq!(View::Untagged.select(&notes), [id(3)]);
    assert_eq!(View::Pinned.select(&notes), [id(2)]);
    assert_eq!(View::Archive.select(&notes), [id(4)]);
    assert!(View::Trash.select(&notes).is_empty());

    // Move `work` under `clients`; archived notes are rewritten too.
    let changes = rename(&notes, "work", "clients/work").unwrap();
    assert_eq!(changes.len(), 2);
    apply(&mut notes, changes, Timestamp(10));
    assert_eq!(notes[0].body(), "# Standup\n#clients/work/acme notes");
    assert_eq!(notes[3].body(), "Old project #clients/work/beta");
    assert_eq!(notes[0].updated(), Timestamp(10));
    assert_eq!(notes[1].updated(), Timestamp(2));

    let tree = TagTree::build(&notes);
    let acme = tree
        .find(&TagPath::parse("Clients/Work/ACME").unwrap())
        .unwrap();
    assert_eq!(acme.notes, [id(1)]);
    assert!(tree.find(&TagPath::parse("work").unwrap()).is_none());

    // Trash and restore keeps the earlier state.
    notes[1].set_trashed(true);
    assert_eq!(View::Trash.select(&notes), [id(2)]);
    assert!(View::Pinned.select(&notes).is_empty());
    notes[1].set_trashed(false);
    assert_eq!(View::Pinned.select(&notes), [id(2)]);
}

#[test]
fn identifiers_round_trip_through_text() {
    let note = NoteId::from_parts(1_700_000_000_000, 99);
    assert_eq!(note.to_string().parse::<NoteId>(), Ok(note));
    let vault = VaultId::from_parts(1_700_000_000_000, 99);
    assert_eq!(vault.to_string().parse::<VaultId>(), Ok(vault));
    assert_eq!(note.to_string(), vault.to_string());
}

#[test]
fn storing_a_vault_locally() {
    let vault = VaultId::from_parts(1_000, 1);
    let vault_key = VaultKey::generate().unwrap();
    let note_key = NoteKey::generate().unwrap();
    let note = id(1);
    let wrapped = vault_key.wrap_note_key(vault, note, &note_key).unwrap();
    let update = note_key
        .encrypt(vault, note, PayloadKind::CrdtUpdate, b"first edit")
        .unwrap();

    let mut store = MemoryStorage::new(vault);
    assert_eq!(store.vault_id(), vault);
    store
        .create_note(
            note,
            &wrapped,
            LocalUpdate {
                id: UpdateId([1; 16]),
                bytes: update.clone(),
            },
        )
        .unwrap();
    let pending: Vec<OutboxEntry> = store.outbox().unwrap();
    assert_eq!(pending[0].bytes, update);
    store.acknowledge(UpdateId([1; 16]), ServerSeq(1)).unwrap();

    // Another device's snapshot arrives.
    let snapshot = note_key
        .encrypt(vault, note, PayloadKind::CrdtSnapshot, b"full state")
        .unwrap();
    store
        .apply_remote(RemoteBatch {
            keys: vec![],
            entries: vec![RemoteEntry {
                note,
                kind: EntryKind::Snapshot,
                id: None,
                seq: ServerSeq(2),
                bytes: snapshot,
            }],
            cursor: ServerSeq(2),
        })
        .unwrap();

    // Load and decrypt everything, as a client does at startup.
    let stored: StoredNote = store.load_note(note).unwrap().unwrap();
    let note_key = vault_key
        .unwrap_note_key(vault, note, &stored.wrapped_key)
        .unwrap();
    let plain: Vec<Vec<u8>> = stored
        .entries
        .iter()
        .map(|e: &LogEntry| {
            let kind = match e.kind {
                EntryKind::Update => PayloadKind::CrdtUpdate,
                EntryKind::Snapshot => PayloadKind::CrdtSnapshot,
            };
            note_key
                .decrypt(vault, note, kind, &e.bytes)
                .unwrap()
                .to_vec()
        })
        .collect();
    assert_eq!(plain, [b"first edit".to_vec(), b"full state".to_vec()]);

    let last: LogPos = stored.entries[1].pos;
    store.compact(note, b"compacted", last).unwrap();
    assert!(store.load_note(note).unwrap().unwrap().entries.is_empty());
    assert_eq!(
        store.append_local(
            id(2),
            LocalUpdate {
                id: UpdateId([2; 16]),
                bytes: vec![]
            }
        ),
        Err(StorageError::NoteNotFound(id(2)))
    );
    store.clear().unwrap();
}

#[test]
fn editing_through_the_editor_exchange() {
    use notebook_core::document::{EditorSync, EditorUpdate, NoteDocument};
    use notebook_core::sync::{EditorSessionId, Replica, ReplicaError};
    use yrs::updates::decoder::Decode;
    use yrs::{GetString, Text, Transact, Update};

    // The editor's replica: a Yjs-compatible doc bound to `body`.
    let editor = yrs::Doc::new();
    let body = editor.get_or_insert_text("body");
    let load = |update: &EditorUpdate| {
        let update = Update::decode_v1(update.as_bytes()).unwrap();
        editor.transact_mut().apply_update(update).unwrap();
    };
    let type_at_end = |text: &str| {
        let mut txn = editor.transact_mut();
        let end = body.len(&txn);
        body.insert(&mut txn, end, text);
        EditorUpdate::from_bytes(txn.encode_update_v1())
    };

    let vault: VaultId = "01ARZ3NDEKTSV4RRFFQ69G5FAV".parse().unwrap();
    let store = MemoryStorage::new(vault);
    let mut replica = Replica::open(store, VaultKey::generate().unwrap()).unwrap();
    let note = replica.create_note("Hi", Timestamp(1)).unwrap();
    let (session, state): (EditorSessionId, EditorUpdate) = replica.editor_attach(note).unwrap();
    load(&state);
    assert_eq!(body.get_string(&editor.transact()), "Hi");

    assert_eq!(
        replica.apply_editor_update(session, &type_at_end("!"), Timestamp(2)),
        Ok(true)
    );
    assert_eq!(replica.note(note).unwrap().body(), "Hi!");
    assert_eq!(replica.pending().unwrap(), 2);
    load(&replica.editor_pending(session).unwrap().unwrap());
    replica.editor_detach(session);
    assert_eq!(
        replica.editor_pending(session),
        Err(ReplicaError::EditorSessionNotFound(session))
    );

    // The document-level building blocks are public too.
    let (mut doc, _) = NoteDocument::create(
        vault,
        id(9),
        NoteKey::generate().unwrap(),
        "x",
        Timestamp(1),
    )
    .unwrap();
    let (_, mut sync): (EditorUpdate, EditorSync) = doc.editor_attach();
    assert_eq!(doc.editor_pending(&mut sync), None);
    assert!(
        doc.apply_editor_update(&mut sync, &EditorUpdate::from_bytes(vec![1]), Timestamp(2))
            .is_err()
    );
}
