//! Behavior that only exists for a store on disk.

use std::fs;
use std::path::{Path, PathBuf};

use notebook_core::crypto::{NoteKey, PayloadKind, VaultId, VaultKey};
use notebook_core::note::NoteId;
use notebook_core::storage::{LocalUpdate, Storage, StorageError};
use notebook_storage_sqlite::SqliteStorage;
use tempfile::TempDir;

fn vault(n: u128) -> VaultId {
    VaultId::from_parts(1_000, n)
}

fn note(n: u128) -> NoteId {
    NoteId::from_parts(1_000, n)
}

fn local(n: u8) -> LocalUpdate {
    LocalUpdate { bytes: vec![n; 4] }
}

fn store_path() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notebook.sqlite3");
    (dir, path)
}

/// Every file in `dir` with its contents.
fn snapshot_dir(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let path = e.unwrap().path();
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    files.sort();
    files
}

#[test]
fn reopen_for_same_vault() {
    let (_dir, path) = store_path();
    let mut s = SqliteStorage::open(&path, vault(1)).unwrap();
    s.create_note(note(1), b"key", local(1)).unwrap();
    let stored = s.load_note(note(1)).unwrap();
    drop(s);

    let s = SqliteStorage::open(&path, vault(1)).unwrap();
    assert_eq!(s.vault_id(), vault(1));
    assert_eq!(s.load_note(note(1)).unwrap(), stored);
}

#[test]
fn open_for_different_vault() {
    let (_dir, path) = store_path();
    let mut s = SqliteStorage::open(&path, vault(1)).unwrap();
    s.create_note(note(1), b"key", local(1)).unwrap();
    let stored = s.load_note(note(1)).unwrap();
    drop(s);

    assert_eq!(
        SqliteStorage::open(&path, vault(2)).unwrap_err(),
        StorageError::VaultMismatch {
            stored: vault(1),
            requested: vault(2),
        }
    );
    let s = SqliteStorage::open(&path, vault(1)).unwrap();
    assert_eq!(s.load_note(note(1)).unwrap(), stored);
}

#[test]
fn open_for_another_vault_after_deleting_the_file() {
    let (_dir, path) = store_path();
    let mut s = SqliteStorage::open(&path, vault(1)).unwrap();
    s.create_note(note(1), b"key", local(1)).unwrap();
    drop(s);
    SqliteStorage::delete_database(&path).unwrap();

    let s = SqliteStorage::open(&path, vault(2)).unwrap();
    assert_eq!(s.vault_id(), vault(2));
    assert!(s.list_notes().unwrap().is_empty());
    drop(s);
    assert!(matches!(
        SqliteStorage::open(&path, vault(1)),
        Err(StorageError::VaultMismatch { .. })
    ));
}

#[test]
fn delete_database_removes_every_file() {
    let (dir, path) = store_path();
    let mut s = SqliteStorage::open(&path, vault(1)).unwrap();
    s.create_note(note(1), b"key", local(1)).unwrap();
    s.set_setting("editor.vim", "true").unwrap();
    drop(s);

    SqliteStorage::delete_database(&path).unwrap();
    assert!(snapshot_dir(dir.path()).is_empty());
    SqliteStorage::delete_database(&path).unwrap();

    let s = SqliteStorage::open(&path, vault(2)).unwrap();
    assert_eq!(s.setting("editor.vim").unwrap(), None);
}

#[test]
fn settings_survive_reopen() {
    let (_dir, path) = store_path();
    let mut s = SqliteStorage::open(&path, vault(1)).unwrap();
    s.set_setting("editor.vim", "true").unwrap();
    drop(s);
    let s = SqliteStorage::open(&path, vault(1)).unwrap();
    assert_eq!(s.setting("editor.vim").unwrap().as_deref(), Some("true"));
    assert_eq!(s.setting("editor.theme").unwrap(), None);
}

#[test]
fn newer_layout_is_refused_untouched() {
    let (dir, path) = store_path();
    let mut s = SqliteStorage::open(&path, vault(1)).unwrap();
    s.create_note(note(1), b"key", local(1)).unwrap();
    drop(s);
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.pragma_update(None, "user_version", 99).unwrap();
    drop(conn);
    let before = snapshot_dir(dir.path());

    assert_eq!(
        SqliteStorage::open(&path, vault(1)).unwrap_err(),
        StorageError::SchemaTooNew(99)
    );
    assert_eq!(snapshot_dir(dir.path()), before);
}

#[test]
fn garbage_file_is_corrupt_and_untouched() {
    let (dir, path) = store_path();
    let garbage: Vec<u8> = b"this is not a sqlite database. "
        .iter()
        .copied()
        .cycle()
        .take(8192)
        .collect();
    fs::write(&path, &garbage).unwrap();
    let before = snapshot_dir(dir.path());

    assert_eq!(
        SqliteStorage::open(&path, vault(1)).unwrap_err(),
        StorageError::Corrupt
    );
    assert_eq!(snapshot_dir(dir.path()), before);
}

#[test]
fn foreign_sqlite_file_is_corrupt_and_untouched() {
    for table in ["meta (a INTEGER)", "contacts (name TEXT)"] {
        let (dir, path) = store_path();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(&format!("CREATE TABLE {table};"))
            .unwrap();
        drop(conn);
        let before = snapshot_dir(dir.path());

        assert_eq!(
            SqliteStorage::open(&path, vault(1)).unwrap_err(),
            StorageError::Corrupt
        );
        assert_eq!(snapshot_dir(dir.path()), before);
    }
}

#[test]
fn update_survives_restart() {
    let (_dir, path) = store_path();
    let mut s = SqliteStorage::open(&path, vault(1)).unwrap();
    s.create_note(note(1), b"key", local(1)).unwrap();
    s.append_local(note(1), local(2)).unwrap();
    // No close: the process ends with the store still open.
    std::mem::forget(s);

    let s = SqliteStorage::open(&path, vault(1)).unwrap();
    let stored = s.load_note(note(1)).unwrap().unwrap();
    let bytes: Vec<_> = stored.entries.iter().map(|e| e.bytes.clone()).collect();
    assert_eq!(bytes, [local(1).bytes, local(2).bytes]);
}

#[test]
fn plaintext_never_reaches_disk() {
    const MARKER: &[u8] = b"MARKER-7f3a9c-plaintext-body";
    let (dir, path) = store_path();
    let v = vault(1);
    let id = note(1);
    let vault_key = VaultKey::generate().unwrap();
    let note_key = NoteKey::generate().unwrap();
    let wrapped = vault_key.wrap_note_key(v, id, &note_key).unwrap();
    let body = |n: u8| [MARKER, &[n]].concat();
    let encrypt = |kind, n| note_key.encrypt(v, id, kind, &body(n)).unwrap();

    let mut s = SqliteStorage::open(&path, v).unwrap();
    s.create_note(
        id,
        &wrapped,
        LocalUpdate {
            bytes: encrypt(PayloadKind::CrdtUpdate, 1),
        },
    )
    .unwrap();
    s.append_local(
        id,
        LocalUpdate {
            bytes: encrypt(PayloadKind::CrdtUpdate, 2),
        },
    )
    .unwrap();
    s.append_local(
        id,
        LocalUpdate {
            bytes: encrypt(PayloadKind::CrdtUpdate, 3),
        },
    )
    .unwrap();
    let through = s
        .load_note(id)
        .unwrap()
        .unwrap()
        .entries
        .last()
        .unwrap()
        .pos;
    s.compact(id, &encrypt(PayloadKind::CrdtSnapshot, 4), through)
        .unwrap();

    let contains_marker = |files: &[(PathBuf, Vec<u8>)]| {
        files
            .iter()
            .any(|(_, bytes)| bytes.windows(MARKER.len()).any(|w| w == MARKER))
    };
    // While open, the WAL holds the recent writes.
    let open_files = snapshot_dir(dir.path());
    assert!(
        open_files
            .iter()
            .any(|(p, _)| p.ends_with("notebook.sqlite3-wal"))
    );
    assert!(!contains_marker(&open_files));
    drop(s);
    assert!(!contains_marker(&snapshot_dir(dir.path())));
}
