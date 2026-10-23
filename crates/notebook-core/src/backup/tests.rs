use super::*;
use crate::crypto::{ENVELOPE_OVERHEAD, NoteKey};
use crate::note::Note;
use crate::storage::{MemoryStorage, Storage};

const CREATED: Timestamp = Timestamp(1_792_677_789_000);
const MARKER: &str = "marker-7f3a9c1e-not-in-the-file";

struct Fixture {
    vault: VaultId,
    vault_key: VaultKey,
    recovery: RecoveryKey,
    entry: RecoveryEntry,
    notes: Vec<BackupNote>,
    /// What each backed-up note reads as, in `notes` order.
    expected: Vec<Note>,
}

fn fixture_for(vault: VaultId) -> Fixture {
    let vault_key = VaultKey::generate().unwrap();
    let recovery = RecoveryKey::generate().unwrap();
    let entry = RecoveryEntry::create(vault, &vault_key, &recovery).unwrap();
    let mut notes = Vec::new();
    let mut expected = Vec::new();
    // Active, pinned, archived, and trashed notes.
    for (n, body) in ["plain note #tag", "pinned", "archived #a/b", MARKER]
        .into_iter()
        .enumerate()
    {
        let id = NoteId::from_parts(2_000, n as u128 + 1);
        let key = NoteKey::generate().unwrap();
        let wrapped_key = vault_key.wrap_note_key(vault, id, &key).unwrap();
        let (mut doc, _) =
            NoteDocument::create(vault, id, key, body, Timestamp(1_000 + n as i64)).unwrap();
        match n {
            1 => drop(doc.set_pinned(true).unwrap()),
            2 => drop(doc.set_archived(true).unwrap()),
            3 => {
                doc.set_pinned(true).unwrap();
                doc.set_trashed(true).unwrap();
                doc.set_body(&format!("{MARKER} edited"), Timestamp(5_000))
                    .unwrap();
            }
            _ => {}
        }
        expected.push(doc.note().unwrap());
        notes.push(BackupNote {
            id,
            wrapped_key,
            snapshot: doc.snapshot().unwrap(),
        });
    }
    Fixture {
        vault,
        vault_key,
        recovery,
        entry,
        notes,
        expected,
    }
}

fn fixture() -> Fixture {
    fixture_for(VaultId::from_parts(1_000, 1))
}

impl Fixture {
    fn build(&self) -> Vec<u8> {
        build(&self.entry, &self.vault_key, CREATED, &self.notes).unwrap()
    }

    fn header_len(&self) -> usize {
        FIXED_HEADER_LEN + self.entry.to_bytes().len()
    }

    /// A file with this fixture's header and a payload sealed around `body`
    /// (everything after the header hash), for malformed payloads.
    fn with_payload(&self, body: &[u8]) -> Vec<u8> {
        let mut file = self.build()[..self.header_len()].to_vec();
        let mut plaintext = Sha256::digest(&file).to_vec();
        plaintext.extend_from_slice(body);
        file.extend(self.vault_key.seal_backup(self.vault, &plaintext).unwrap());
        file
    }
}

/// Reads a note back the way restore does: key unwrapped by the vault key,
/// snapshot loaded into a fresh document.
fn read_note(vault: VaultId, vault_key: &VaultKey, note: &BackupNote) -> Note {
    let key = vault_key
        .unwrap_note_key(vault, note.id, &note.wrapped_key)
        .unwrap();
    let mut doc = NoteDocument::open(vault, note.id, key).unwrap();
    doc.load_snapshot(note.snapshot.as_bytes()).unwrap();
    doc.note().unwrap()
}

#[test]
fn round_trip_with_the_recovery_key() {
    let f = fixture();
    let file = f.build();

    let header = read_header(&file).unwrap();
    assert_eq!(header.version, FORMAT_VERSION);
    assert_eq!(header.vault, f.vault);
    assert_eq!(header.recovery, f.entry);

    let vault_key = header.vault_key(&f.recovery).unwrap();
    let opened = open(&file, &vault_key).unwrap();
    assert_eq!(opened.vault, f.vault);
    assert_eq!(opened.created_at, CREATED);
    assert_eq!(opened.notes, f.notes);
    opened.verify(&vault_key).unwrap();

    let restored: Vec<_> = opened
        .notes
        .iter()
        .map(|n| read_note(f.vault, &vault_key, n))
        .collect();
    assert_eq!(restored, f.expected);
    assert!(restored[1].is_pinned());
    assert!(restored[2].is_archived());
    assert!(restored[3].is_trashed() && restored[3].is_pinned());
}

#[test]
fn imported_into_a_fresh_store_loads_the_same_notes() {
    let f = fixture();
    let opened = open(&f.build(), &f.vault_key).unwrap();
    let mut store = MemoryStorage::new(opened.vault);
    for note in &opened.notes {
        store
            .import_note(note.id, &note.wrapped_key, note.snapshot.as_bytes())
            .unwrap();
    }
    let ids: Vec<_> = f.notes.iter().map(|n| n.id).collect();
    assert_eq!(store.list_notes().unwrap(), ids);
    for (id, expected) in ids.into_iter().zip(&f.expected) {
        let stored = store.load_note(id).unwrap().unwrap();
        assert!(stored.entries.is_empty());
        let note = BackupNote {
            id,
            wrapped_key: stored.wrapped_key,
            snapshot: EncryptedSnapshot::from_bytes(stored.base.unwrap()),
        };
        assert_eq!(&read_note(f.vault, &f.vault_key, &note), expected);
    }
}

#[test]
fn empty_notebook_round_trips() {
    let f = fixture();
    let file = build(&f.entry, &f.vault_key, CREATED, &[]).unwrap();
    let opened = open(&file, &f.vault_key).unwrap();
    assert!(opened.notes.is_empty());
    opened.verify(&f.vault_key).unwrap();
}

#[test]
fn wrong_recovery_key() {
    let f = fixture();
    let header = read_header(&f.build()).unwrap();
    let other = RecoveryKey::generate().unwrap();
    assert_eq!(header.vault_key(&other).unwrap_err(), BackupError::WrongKey);
}

#[test]
fn wrong_vault_key_is_damaged() {
    let f = fixture();
    let other = VaultKey::generate().unwrap();
    assert_eq!(open(&f.build(), &other), Err(BackupError::Damaged));
}

#[test]
fn marker_text_and_note_ids_not_in_the_file() {
    let f = fixture();
    let file = f.build();
    let contains = |needle: &[u8]| file.windows(needle.len()).any(|w| w == needle);
    assert!(!contains(MARKER.as_bytes()));
    assert!(!contains(b"plain note"));
    assert!(!contains(b"#tag"));
    for note in &f.notes {
        let bytes = note.id.to_bytes();
        let text = note.id.to_string();
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        for encoding in [
            bytes.to_vec(),
            text.clone().into_bytes(),
            text.to_lowercase().into_bytes(),
            hex.clone().into_bytes(),
            hex.to_uppercase().into_bytes(),
        ] {
            assert!(!contains(&encoding), "note {text} appears in the file");
        }
        // Wrapped keys and snapshots are inside the payload too.
        assert!(!contains(&note.wrapped_key));
        assert!(!contains(note.snapshot.as_bytes()));
    }
}

#[test]
fn every_single_byte_flip_fails() {
    let f = fixture();
    let file = f.build();
    for i in 0..file.len() {
        for bit in [0x01, 0x80] {
            let mut bad = file.clone();
            bad[i] ^= bit;
            let err = open(&bad, &f.vault_key).unwrap_err();
            match i {
                0..8 => assert_eq!(err, BackupError::NotABackup, "byte {i}"),
                // Version 1 becomes 0 or 129.
                8 => assert!(
                    matches!(err, BackupError::Damaged | BackupError::TooNew(129)),
                    "byte {i}: {err:?}"
                ),
                _ => assert_eq!(err, BackupError::Damaged, "byte {i}"),
            }
        }
    }
}

#[test]
fn every_truncation_fails() {
    let f = fixture();
    let file = f.build();
    for len in 0..file.len() {
        let err = open(&file[..len], &f.vault_key).unwrap_err();
        let expected = if len == 0 {
            BackupError::NotABackup
        } else {
            BackupError::Damaged
        };
        assert_eq!(err, expected, "length {len}");
    }
}

#[test]
fn appended_bytes_fail() {
    let f = fixture();
    let mut file = f.build();
    file.push(0);
    assert_eq!(open(&file, &f.vault_key), Err(BackupError::Damaged));
}

#[test]
fn header_vault_swapped() {
    let a = fixture();
    let b = fixture_for(VaultId::from_parts(1_000, 2));
    let file = a.build();
    let header_len = a.header_len();

    // Only the header's vault ID changed: it no longer matches the entry.
    let mut bad = file.clone();
    bad[9..25].copy_from_slice(&b.vault.to_bytes());
    assert_eq!(read_header(&bad), Err(BackupError::Damaged));

    // Header and entry both claim vault B: the payload is still bound to A.
    let mut bad = file.clone();
    bad[9..25].copy_from_slice(&b.vault.to_bytes());
    bad[28..44].copy_from_slice(&b.vault.to_bytes());
    assert_eq!(read_header(&bad).unwrap().vault, b.vault);
    assert_eq!(open(&bad, &a.vault_key), Err(BackupError::Damaged));

    // B's whole header in front of A's payload.
    let mut spliced = b.build()[..b.header_len()].to_vec();
    spliced.extend_from_slice(&file[header_len..]);
    assert_eq!(open(&spliced, &a.vault_key), Err(BackupError::Damaged));
    assert_eq!(open(&spliced, &b.vault_key), Err(BackupError::Damaged));

    // A's header with B's recovery entry, same vault ID otherwise.
    let mut swapped = a.build();
    let b_file = b.build();
    swapped[27..header_len].copy_from_slice(&b_file[27..b.header_len()]);
    swapped[9..25].copy_from_slice(&b.vault.to_bytes());
    assert_eq!(open(&swapped, &a.vault_key), Err(BackupError::Damaged));
}

#[test]
fn not_a_backup() {
    let f = fixture();
    for bytes in [
        &b""[..],
        b"# My note\n\nSome markdown.\n",
        b"NBBACKUQ\x01",
        b"nbbackup\x01",
        b"x",
    ] {
        assert_eq!(
            read_header(bytes),
            Err(BackupError::NotABackup),
            "{bytes:?}"
        );
        assert_eq!(open(bytes, &f.vault_key), Err(BackupError::NotABackup));
    }
}

#[test]
fn newer_format() {
    let f = fixture();
    let mut file = f.build();
    for version in [2, 9, u8::MAX] {
        file[8] = version;
        assert_eq!(read_header(&file), Err(BackupError::TooNew(version)));
        assert_eq!(open(&file, &f.vault_key), Err(BackupError::TooNew(version)));
        // Just the magic and version byte is enough to tell.
        assert_eq!(read_header(&file[..9]), Err(BackupError::TooNew(version)));
    }
    file[8] = 0;
    assert_eq!(read_header(&file), Err(BackupError::Damaged));
}

#[test]
fn truncated_headers() {
    let f = fixture();
    let file = f.build();
    let header_len = f.header_len();
    for len in 1..header_len {
        assert_eq!(
            read_header(&file[..len]),
            Err(BackupError::Damaged),
            "length {len}"
        );
    }
    // The header alone is enough to read it, though not to open.
    assert_eq!(read_header(&file[..header_len]).unwrap().vault, f.vault);
    assert_eq!(
        open(&file[..header_len], &f.vault_key),
        Err(BackupError::Damaged)
    );
}

#[test]
fn recovery_entry_length_must_fit_the_entry() {
    let f = fixture();
    let file = f.build();
    for len in [0u16, 1, 90, 92, u16::MAX] {
        let mut bad = file.clone();
        bad[25..27].copy_from_slice(&len.to_be_bytes());
        assert_eq!(read_header(&bad), Err(BackupError::Damaged), "length {len}");
    }
}

#[test]
fn payload_with_trailing_bytes_is_damaged() {
    let f = fixture();
    let mut body = CREATED.as_millis().to_be_bytes().to_vec();
    body.extend_from_slice(&0u32.to_be_bytes());
    assert!(open(&f.with_payload(&body), &f.vault_key).is_ok());
    body.push(0);
    assert_eq!(
        open(&f.with_payload(&body), &f.vault_key),
        Err(BackupError::Damaged)
    );
}

#[test]
fn huge_note_count_is_damaged() {
    let f = fixture();
    let mut body = CREATED.as_millis().to_be_bytes().to_vec();
    body.extend_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(
        open(&f.with_payload(&body), &f.vault_key),
        Err(BackupError::Damaged)
    );
}

#[test]
fn oversized_lengths_are_damaged() {
    let f = fixture();
    let note = &f.notes[0];
    let mut body = CREATED.as_millis().to_be_bytes().to_vec();
    body.extend_from_slice(&1u32.to_be_bytes());
    body.extend_from_slice(&note.id.to_bytes());
    body.extend_from_slice(&u16::MAX.to_be_bytes());
    body.extend_from_slice(&note.wrapped_key);
    assert_eq!(
        open(&f.with_payload(&body), &f.vault_key),
        Err(BackupError::Damaged)
    );
}

#[test]
fn bad_header_hash_is_damaged() {
    let f = fixture();
    let mut file = f.build()[..f.header_len()].to_vec();
    let mut plaintext = vec![0u8; HASH_LEN];
    plaintext.extend_from_slice(&CREATED.as_millis().to_be_bytes());
    plaintext.extend_from_slice(&0u32.to_be_bytes());
    file.extend(f.vault_key.seal_backup(f.vault, &plaintext).unwrap());
    assert_eq!(open(&file, &f.vault_key), Err(BackupError::Damaged));
}

#[test]
fn duplicate_note_ids_are_damaged() {
    let f = fixture();
    let notes = [f.notes[0].clone(), f.notes[0].clone()];
    let file = build(&f.entry, &f.vault_key, CREATED, &notes).unwrap();
    assert_eq!(open(&file, &f.vault_key), Err(BackupError::Damaged));
}

#[test]
fn verify_catches_bad_notes() {
    let f = fixture();
    let check = |notes: &[BackupNote]| {
        let file = build(&f.entry, &f.vault_key, CREATED, notes).unwrap();
        open(&file, &f.vault_key).unwrap().verify(&f.vault_key)
    };
    assert_eq!(check(&f.notes), Ok(()));

    // A snapshot that isn't ciphertext.
    let mut notes = f.notes.clone();
    notes[2].snapshot = EncryptedSnapshot::from_bytes(b"garbage".to_vec());
    assert_eq!(check(&notes), Err(BackupError::Damaged));

    // Another note's snapshot: bound to the other note's ID and key.
    let mut notes = f.notes.clone();
    notes[0].snapshot = f.notes[1].snapshot.clone();
    assert_eq!(check(&notes), Err(BackupError::Damaged));

    // A key wrapped by another vault key.
    let mut notes = f.notes.clone();
    let other = VaultKey::generate().unwrap();
    notes[1].wrapped_key = other
        .wrap_note_key(f.vault, notes[1].id, &NoteKey::generate().unwrap())
        .unwrap();
    assert_eq!(check(&notes), Err(BackupError::Damaged));

    // A valid snapshot of a document that never got its creating update.
    let mut notes = f.notes.clone();
    let id = notes[3].id;
    let key = NoteKey::generate().unwrap();
    notes[3].wrapped_key = f.vault_key.wrap_note_key(f.vault, id, &key).unwrap();
    notes[3].snapshot = NoteDocument::open(f.vault, id, key)
        .unwrap()
        .snapshot()
        .unwrap();
    assert_eq!(check(&notes), Err(BackupError::Damaged));
}

/// Parses a built file using only the offsets in the module docs and
/// `docs/backup-format.md`.
#[test]
fn documented_layout() {
    let f = fixture();
    let file = f.build();

    assert_eq!(&file[0..8], b"NBBACKUP");
    assert_eq!(file[8], 1);
    assert_eq!(&file[9..25], &f.vault.to_bytes());
    let r = u16::from_be_bytes([file[25], file[26]]) as usize;
    assert_eq!(r, 91);
    assert_eq!(&file[27..27 + r], &f.entry.to_bytes()[..]);
    // The recovery entry: version 1, vault ID, 74-byte envelope.
    assert_eq!(file[27], 1);
    assert_eq!(&file[28..44], &f.vault.to_bytes());

    let payload = &file[27 + r..];
    // Envelope version 1, XChaCha20-Poly1305.
    assert_eq!(&payload[..2], &[1, 1]);
    let plaintext = f.vault_key.open_backup(f.vault, payload).unwrap();
    assert_eq!(payload.len(), plaintext.len() + ENVELOPE_OVERHEAD);

    assert_eq!(&plaintext[..32], Sha256::digest(&file[..27 + r]).as_slice());
    let created = i64::from_be_bytes(plaintext[32..40].try_into().unwrap());
    assert_eq!(created, CREATED.as_millis());
    let count = u32::from_be_bytes(plaintext[40..44].try_into().unwrap()) as usize;
    assert_eq!(count, f.notes.len());
    let mut at = 44;
    for note in &f.notes {
        assert_eq!(&plaintext[at..at + 16], &note.id.to_bytes());
        at += 16;
        let k = u16::from_be_bytes([plaintext[at], plaintext[at + 1]]) as usize;
        at += 2;
        assert_eq!(&plaintext[at..at + k], &note.wrapped_key[..]);
        at += k;
        let s = u32::from_be_bytes(plaintext[at..at + 4].try_into().unwrap()) as usize;
        at += 4;
        assert_eq!(&plaintext[at..at + s], note.snapshot.as_bytes());
        at += s;
    }
    assert_eq!(at, plaintext.len());
}

#[test]
fn error_messages() {
    let cases = [
        (BackupError::NotABackup, "not a Notebook backup"),
        (
            BackupError::TooNew(2),
            "backup format 2 needs a newer version of Notebook",
        ),
        (BackupError::Damaged, "backup is damaged"),
        (
            BackupError::WrongKey,
            "the recovery key does not open this backup",
        ),
        (
            BackupError::TooLarge,
            "note is too large for the backup format",
        ),
        (
            BackupError::Crypto(CryptoError::Random),
            "backup encryption failed: secure random source failed",
        ),
    ];
    for (err, message) in cases {
        assert_eq!(err.to_string(), message);
    }
}
