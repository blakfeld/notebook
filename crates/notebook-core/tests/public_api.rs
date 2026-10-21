//! End-to-end flows through the public API only, as a client would use it.

use notebook_core::crypto::{
    AuthSecret, CryptoError, KdfVersion, KeyBundle, NoteKey, PayloadKind, RecoveryKey, Salt,
    VaultId, VaultKey, derive_auth_secret,
};
use notebook_core::note::{Note, NoteId, Timestamp};
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
    let vault_key = bundle.unlock_with_recovery(&recovery).unwrap();
    let salt = Salt::generate().unwrap();
    let new_auth = derive_auth_secret(b"new passphrase", &salt, KdfVersion::CURRENT).unwrap();
    bundle.rewrap_auth(&vault_key, &new_auth).unwrap();

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
