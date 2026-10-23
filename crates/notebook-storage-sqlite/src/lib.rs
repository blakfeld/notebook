//! SQLite local storage for native Notebook clients.
//!
//! [`SqliteStorage`] implements [`Storage`] on one SQLite file using the
//! bundled SQLite. It passes the same conformance suite as
//! [`MemoryStorage`](notebook_core::storage::MemoryStorage).
//!
//! - **Path.** The app passes one fixed per-device path, such as
//!   `<app data dir>/notebook.sqlite3`. SQLite keeps `-wal` and `-shm` files
//!   beside it.
//! - **One vault at a time.** The file is bound to the vault it was first
//!   opened for. Opening it for another vault fails with
//!   [`StorageError::VaultMismatch`]; [`SqliteStorage::delete_database`]
//!   removes the files so the path can be used again.
//! - **Settings are unencrypted.** Note content and keys are stored only as
//!   ciphertext and wrapped keys, but settings are plain text, so they must
//!   hold only non-sensitive values. Note IDs, the vault ID, and sizes are
//!   readable from the file.
//! - **Corruption is never repaired.** A file that isn't a valid store fails
//!   with [`StorageError::Corrupt`] and is left untouched, because it may
//!   hold the only copy of some edits. Resetting is the app's call.
//! - **Recovery entry.** The vault's [`RecoveryEntry`] is kept in the file
//!   ([`SqliteStorage::recovery_entry`]). It opens only with the recovery
//!   key, so it isn't secret on its own.
//! - **Durability.** Writes use WAL with `synchronous=FULL`: once a call
//!   returns, its change survives a crash or power loss. Each call is one
//!   transaction.

#![forbid(unsafe_code)]

use std::io;
use std::path::Path;

use notebook_core::crypto::{RecoveryEntry, VaultId};
use notebook_core::note::NoteId;
use notebook_core::storage::{
    EntryKind, LocalUpdate, LogEntry, LogPos, Result, Storage, StorageError, StoredNote,
};
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, Transaction, params};

mod schema;

/// A [`Storage`] backed by a SQLite file.
#[derive(Debug)]
pub struct SqliteStorage {
    conn: Connection,
    vault: VaultId,
}

/// What [`SqliteStorage::peek`] finds in a store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredVault {
    pub vault: VaultId,
    pub recovery_entry: Option<RecoveryEntry>,
}

/// Maps SQLite errors to [`StorageError`].
trait Db<T> {
    fn db(self) -> Result<T>;
}

impl<T> Db<T> for rusqlite::Result<T> {
    fn db(self) -> Result<T> {
        self.map_err(|e| match e.sqlite_error_code() {
            Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase) => StorageError::Corrupt,
            _ => StorageError::Backend(e.to_string()),
        })
    }
}

const KIND_UPDATE: u8 = 0;
const KIND_SNAPSHOT: u8 = 1;

fn kind_from_sql(value: u8) -> Result<EntryKind> {
    match value {
        KIND_UPDATE => Ok(EntryKind::Update),
        KIND_SNAPSHOT => Ok(EntryKind::Snapshot),
        _ => Err(StorageError::Corrupt),
    }
}

impl SqliteStorage {
    /// Opens or creates the store at `path` for `vault`, upgrading an older
    /// layout in place.
    ///
    /// Fails with [`StorageError::Corrupt`] if the file isn't a valid store,
    /// [`StorageError::SchemaTooNew`] if a newer client wrote it, or
    /// [`StorageError::VaultMismatch`] if it holds another vault's data. In
    /// those cases the file is not changed.
    pub fn open(path: impl AsRef<Path>, vault: VaultId) -> Result<Self> {
        Self::open_with(Connection::open(path).db()?, vault, schema::MIGRATIONS)
    }

    /// Opens a fresh store that lives only in memory, for tests.
    pub fn open_in_memory(vault: VaultId) -> Result<Self> {
        Self::open_with(
            Connection::open_in_memory().db()?,
            vault,
            schema::MIGRATIONS,
        )
    }

    fn open_with(mut conn: Connection, vault: VaultId, migrations: &[&str]) -> Result<Self> {
        // Connection-only settings first; nothing here writes to the file.
        conn.pragma_update(None, "foreign_keys", true).db()?;
        conn.busy_timeout(std::time::Duration::from_secs(5)).db()?;
        let check: String = conn
            .pragma_query_value(None, "quick_check", |row| row.get(0))
            .db()?;
        if check != "ok" {
            return Err(StorageError::Corrupt);
        }
        let version = schema::version(&conn).db()?;
        if version as usize > migrations.len() {
            return Err(StorageError::SchemaTooNew(version));
        }
        // Version 0 with tables is some other SQLite file, not a new store.
        if version == 0 {
            let objects: u64 = conn
                .query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))
                .db()?;
            if objects > 0 {
                return Err(StorageError::Corrupt);
            }
        }

        let _mode: String = conn
            .pragma_update_and_check(None, "journal_mode", "wal", |row| row.get(0))
            .db()?;
        conn.pragma_update(None, "synchronous", "FULL").db()?;
        schema::migrate(&mut conn, version, migrations).db()?;

        let tx = conn.transaction().db()?;
        let stored: Option<Vec<u8>> = tx
            .query_row("SELECT value FROM meta WHERE key = 'vault_id'", [], |row| {
                row.get(0)
            })
            .optional()
            .db()?;
        match stored {
            Some(bytes) => {
                let bytes: [u8; 16] = bytes.try_into().map_err(|_| StorageError::Corrupt)?;
                let stored = VaultId::from_bytes(bytes);
                if stored != vault {
                    return Err(StorageError::VaultMismatch {
                        stored,
                        requested: vault,
                    });
                }
            }
            None => {
                tx.execute(
                    "INSERT INTO meta (key, value) VALUES ('vault_id', ?1)",
                    [vault.to_bytes()],
                )
                .db()?;
            }
        }
        tx.commit().db()?;
        Ok(Self { conn, vault })
    }

    /// Reads which vault the store at `path` holds, and its recovery entry,
    /// without changing the file: no migration, no vault binding. `None`
    /// when there is no file or it isn't bound to a vault yet.
    pub fn peek(path: impl AsRef<Path>) -> Result<Option<StoredVault>> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(None);
        }
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).db()?;
        conn.busy_timeout(std::time::Duration::from_secs(5)).db()?;
        let has_table = |name: &str| -> Result<bool> {
            conn.query_row(
                "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1",
                [name],
                |_| Ok(()),
            )
            .optional()
            .db()
            .map(|r| r.is_some())
        };
        if !has_table("meta")? {
            return Ok(None);
        }
        let Some(bytes) = conn
            .query_row("SELECT value FROM meta WHERE key = 'vault_id'", [], |row| {
                row.get::<_, Vec<u8>>(0)
            })
            .optional()
            .db()?
        else {
            return Ok(None);
        };
        let bytes: [u8; 16] = bytes.try_into().map_err(|_| StorageError::Corrupt)?;
        let recovery_entry = if has_table("vault_meta")? {
            conn.query_row(
                "SELECT recovery_entry FROM vault_meta WHERE id = 1",
                [],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()
            .db()?
            .map(|b| RecoveryEntry::from_bytes(&b).map_err(|_| StorageError::Corrupt))
            .transpose()?
        } else {
            None
        };
        Ok(Some(StoredVault {
            vault: VaultId::from_bytes(bytes),
            recovery_entry,
        }))
    }

    /// The vault's recovery entry, if one has been stored. Bytes that don't
    /// parse are [`StorageError::Corrupt`].
    pub fn recovery_entry(&self) -> Result<Option<RecoveryEntry>> {
        let bytes: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT recovery_entry FROM vault_meta WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .db()?;
        bytes
            .map(|b| RecoveryEntry::from_bytes(&b).map_err(|_| StorageError::Corrupt))
            .transpose()
    }

    /// Stores `entry`, replacing any earlier one. It must belong to this
    /// store's vault, or the call fails with [`StorageError::VaultMismatch`].
    pub fn set_recovery_entry(&mut self, entry: &RecoveryEntry) -> Result<()> {
        if entry.vault_id() != self.vault {
            return Err(StorageError::VaultMismatch {
                stored: self.vault,
                requested: entry.vault_id(),
            });
        }
        self.conn
            .execute(
                "INSERT INTO vault_meta (id, recovery_entry) VALUES (1, ?1)
                 ON CONFLICT (id) DO UPDATE SET recovery_entry = excluded.recovery_entry",
                [entry.to_bytes()],
            )
            .db()?;
        Ok(())
    }

    /// Removes the store at `path` and its `-wal`, `-shm`, and `-journal` files, settings
    /// included. Missing files are fine. The store must not be open.
    pub fn delete_database(path: impl AsRef<Path>) -> io::Result<()> {
        let path = path.as_ref();
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let mut name = path.as_os_str().to_owned();
            name.push(suffix);
            match std::fs::remove_file(&name) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        Ok(())
    }
}

fn note_exists(tx: &Transaction, id: NoteId) -> Result<bool> {
    tx.query_row(
        "SELECT 1 FROM notes WHERE note_id = ?1",
        [id.to_bytes()],
        |_| Ok(()),
    )
    .optional()
    .db()
    .map(|r| r.is_some())
}

fn insert_local(tx: &Transaction, id: NoteId, update: &LocalUpdate) -> Result<()> {
    tx.execute(
        "INSERT INTO log (note_id, kind, bytes) VALUES (?1, ?2, ?3)",
        params![id.to_bytes(), KIND_UPDATE, update.bytes],
    )
    .db()?;
    Ok(())
}

impl Storage for SqliteStorage {
    fn vault_id(&self) -> VaultId {
        self.vault
    }

    fn list_notes(&self) -> Result<Vec<NoteId>> {
        let mut stmt = self
            .conn
            .prepare("SELECT note_id FROM notes ORDER BY note_id")
            .db()?;
        stmt.query_map([], |row| row.get(0).map(NoteId::from_bytes))
            .db()?
            .collect::<rusqlite::Result<_>>()
            .db()
    }

    fn load_note(&self, id: NoteId) -> Result<Option<StoredNote>> {
        let tx = self.conn.unchecked_transaction().db()?;
        let Some((wrapped_key, base)) = tx
            .query_row(
                "SELECT wrapped_key, base FROM notes WHERE note_id = ?1",
                [id.to_bytes()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .db()?
        else {
            return Ok(None);
        };
        let mut stmt = tx
            .prepare(
                "SELECT pos, kind, bytes FROM log
                 WHERE note_id = ?1 ORDER BY pos",
            )
            .db()?;
        let rows = stmt
            .query_map([id.to_bytes()], |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, u8>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .db()?
            .collect::<rusqlite::Result<Vec<_>>>()
            .db()?;
        let entries = rows
            .into_iter()
            .map(|(pos, kind, bytes)| {
                Ok(LogEntry {
                    pos: LogPos(pos),
                    kind: kind_from_sql(kind)?,
                    bytes,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Some(StoredNote {
            wrapped_key,
            base,
            entries,
        }))
    }

    fn create_note(&mut self, id: NoteId, wrapped_key: &[u8], initial: LocalUpdate) -> Result<()> {
        let tx = self.conn.transaction().db()?;
        if note_exists(&tx, id)? {
            return Err(StorageError::DuplicateNote(id));
        }
        tx.execute(
            "INSERT INTO notes (note_id, wrapped_key, base) VALUES (?1, ?2, NULL)",
            params![id.to_bytes(), wrapped_key],
        )
        .db()?;
        insert_local(&tx, id, &initial)?;
        tx.commit().db()
    }

    fn import_note(&mut self, id: NoteId, wrapped_key: &[u8], snapshot: &[u8]) -> Result<()> {
        let tx = self.conn.transaction().db()?;
        if note_exists(&tx, id)? {
            return Err(StorageError::DuplicateNote(id));
        }
        tx.execute(
            "INSERT INTO notes (note_id, wrapped_key, base) VALUES (?1, ?2, ?3)",
            params![id.to_bytes(), wrapped_key, snapshot],
        )
        .db()?;
        tx.commit().db()
    }

    fn append_local(&mut self, id: NoteId, update: LocalUpdate) -> Result<()> {
        let tx = self.conn.transaction().db()?;
        if !note_exists(&tx, id)? {
            return Err(StorageError::NoteNotFound(id));
        }
        insert_local(&tx, id, &update)?;
        tx.commit().db()
    }

    fn compact(&mut self, id: NoteId, snapshot: &[u8], through: LogPos) -> Result<()> {
        let tx = self.conn.transaction().db()?;
        if !note_exists(&tx, id)? {
            return Err(StorageError::NoteNotFound(id));
        }
        tx.execute(
            "DELETE FROM log WHERE note_id = ?1 AND pos <= ?2",
            params![id.to_bytes(), through.0],
        )
        .db()?;
        tx.execute(
            "UPDATE notes SET base = ?2 WHERE note_id = ?1",
            params![id.to_bytes(), snapshot],
        )
        .db()?;
        tx.commit().db()
    }

    fn delete_note(&mut self, id: NoteId) -> Result<()> {
        // Log rows go with it through ON DELETE CASCADE.
        self.conn
            .execute("DELETE FROM notes WHERE note_id = ?1", [id.to_bytes()])
            .db()?;
        Ok(())
    }

    fn setting(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .db()
    }

    fn set_setting(&mut self, key: &str, value: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                [key, value],
            )
            .db()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use notebook_core::crypto::{NoteKey, RecoveryKey, VaultKey};
    use notebook_core::document::NoteDocument;
    use notebook_core::note::{Note, Timestamp};
    use notebook_core::replica::Replica;

    use super::*;

    fn vault() -> VaultId {
        VaultId::from_parts(1_000, 1)
    }

    fn note(n: u128) -> NoteId {
        NoteId::from_parts(1_000, n)
    }

    fn local(n: u8) -> LocalUpdate {
        LocalUpdate { bytes: vec![n; 4] }
    }

    fn tables(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_schema
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .unwrap();
        stmt.query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    fn columns(conn: &Connection, table: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info(?1) ORDER BY cid")
            .unwrap();
        stmt.query_map([table], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn fresh_store_has_the_latest_layout() {
        let s = SqliteStorage::open_in_memory(vault()).unwrap();
        assert_eq!(schema::version(&s.conn).unwrap(), 3);
        assert_eq!(schema::MIGRATIONS.len(), 3);
        assert_eq!(
            tables(&s.conn),
            ["log", "meta", "notes", "settings", "vault_meta"]
        );
        assert_eq!(columns(&s.conn, "log"), ["pos", "note_id", "kind", "bytes"]);
        assert_eq!(
            columns(&s.conn, "notes"),
            ["note_id", "wrapped_key", "base"]
        );
    }

    fn recovery_entry() -> (RecoveryEntry, RecoveryKey) {
        let recovery = RecoveryKey::generate().unwrap();
        let entry =
            RecoveryEntry::create(vault(), &VaultKey::generate().unwrap(), &recovery).unwrap();
        (entry, recovery)
    }

    #[test]
    fn recovery_entry_round_trips_and_survives_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.sqlite3");
        let mut s = SqliteStorage::open(&path, vault()).unwrap();
        assert_eq!(s.recovery_entry().unwrap(), None);

        let (first, _) = recovery_entry();
        s.set_recovery_entry(&first).unwrap();
        assert_eq!(s.recovery_entry().unwrap().as_ref(), Some(&first));

        let (second, recovery) = recovery_entry();
        s.set_recovery_entry(&second).unwrap();
        drop(s);

        let s = SqliteStorage::open(&path, vault()).unwrap();
        let stored = s.recovery_entry().unwrap().unwrap();
        assert_eq!(stored, second);
        assert!(stored.open(&recovery).is_ok());
    }

    #[test]
    fn recovery_entry_for_another_vault_is_refused() {
        let mut s = SqliteStorage::open_in_memory(vault()).unwrap();
        let entry = RecoveryEntry::create(
            VaultId::from_parts(1_000, 2),
            &VaultKey::generate().unwrap(),
            &RecoveryKey::generate().unwrap(),
        )
        .unwrap();
        assert!(matches!(
            s.set_recovery_entry(&entry),
            Err(StorageError::VaultMismatch { .. })
        ));
        assert_eq!(s.recovery_entry().unwrap(), None);
    }

    #[test]
    fn unparsable_recovery_entry_is_corrupt() {
        let s = SqliteStorage::open_in_memory(vault()).unwrap();
        s.conn
            .execute(
                "INSERT INTO vault_meta (id, recovery_entry) VALUES (1, x'00')",
                [],
            )
            .unwrap();
        assert_eq!(s.recovery_entry(), Err(StorageError::Corrupt));
    }

    #[test]
    fn peek_reads_without_changing_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.sqlite3");
        assert_eq!(SqliteStorage::peek(&path).unwrap(), None);
        assert!(!path.exists());

        // A version-one store: a vault, no recovery entry, and no upgrade.
        let conn = Connection::open(&path).unwrap();
        let s = SqliteStorage::open_with(conn, vault(), &schema::MIGRATIONS[..1]).unwrap();
        drop(s);
        let peeked = SqliteStorage::peek(&path).unwrap().unwrap();
        assert_eq!(peeked.vault, vault());
        assert_eq!(peeked.recovery_entry, None);
        let conn = Connection::open(&path).unwrap();
        assert_eq!(schema::version(&conn).unwrap(), 1);
        drop(conn);

        let mut s = SqliteStorage::open(&path, vault()).unwrap();
        let (entry, _) = recovery_entry();
        s.set_recovery_entry(&entry).unwrap();
        // Peeking while the store is open, as the app never does but may.
        let peeked = SqliteStorage::peek(&path).unwrap().unwrap();
        assert_eq!(peeked.recovery_entry, Some(entry));
        drop(s);
        SqliteStorage::delete_database(&path).unwrap();
        assert_eq!(SqliteStorage::peek(&path).unwrap(), None);
    }

    /// A log row as the sync-era layouts (versions 1 and 2) stored it.
    struct OldRow {
        note: NoteId,
        update_id: Option<[u8; 16]>,
        kind: u8,
        seq: Option<u64>,
        bytes: Vec<u8>,
    }

    /// A note row as the sync-era layouts stored it.
    struct OldNote {
        id: NoteId,
        wrapped_key: Vec<u8>,
        key_pending: bool,
        base: Option<Vec<u8>>,
    }

    /// Notes as a syncing install left them: their decoded state, and the
    /// raw rows to seed.
    struct OldStore {
        vault_key: VaultKey,
        notes: Vec<Note>,
        note_rows: Vec<OldNote>,
        log: Vec<OldRow>,
    }

    impl OldStore {
        fn key(&self) -> VaultKey {
            VaultKey::from_secret_bytes(&self.vault_key.to_secret_bytes()[..]).unwrap()
        }

        /// Position the seeded row `i` gets. The gaps stand for entries
        /// compaction removed.
        fn pos(i: usize) -> u64 {
            10 + 2 * i as u64
        }
    }

    /// Four notes covering every sync state: one created and acknowledged
    /// with an unacknowledged edit after it; one compacted, with an
    /// acknowledged update and a remote snapshot after its base; one that
    /// came from another device; and one never acknowledged at all, with its
    /// key still pending.
    fn old_store() -> OldStore {
        let vault_key = VaultKey::generate().unwrap();
        let t = |n: i64| Timestamp(1_700_000_000_000 + n);
        let mut store = OldStore {
            vault_key,
            notes: Vec::new(),
            note_rows: Vec::new(),
            log: Vec::new(),
        };
        let row = |log: &mut Vec<OldRow>, note, kind: u8, seq: Option<u64>, bytes| {
            let n = log.len() as u8 + 1;
            log.push(OldRow {
                note,
                update_id: (kind == 0).then_some([n; 16]),
                kind,
                seq,
                bytes,
            });
        };
        let create = |store: &mut OldStore, n: u128, body: &str, key_pending, base| {
            let id = note(n);
            let key = NoteKey::generate().unwrap();
            let wrapped_key = store.vault_key.wrap_note_key(vault(), id, &key).unwrap();
            let (doc, first) = NoteDocument::create(vault(), id, key, body, t(0)).unwrap();
            store.note_rows.push(OldNote {
                id,
                wrapped_key,
                key_pending,
                base,
            });
            (id, doc, first.into_bytes())
        };

        // Acknowledged creation, then an edit still in the outbox.
        let (a, mut doc, first) = create(&mut store, 1, "Groceries\n#home", false, None);
        row(&mut store.log, a, 0, Some(1), first);
        let edit = doc.set_body("Groceries\n#home\nmilk", t(1)).unwrap();
        row(&mut store.log, a, 0, None, edit.unwrap().into_bytes());
        store.notes.push(doc.note().unwrap());

        // Compacted: a base, then an acknowledged update and a remote
        // snapshot of a later state.
        let (_, mut doc, _) = create(&mut store, 2, "Standup\n#work/acme", false, None);
        store.note_rows[1].base = Some(doc.snapshot().unwrap().into_bytes());
        let b = note(2);
        let edit = doc.set_body("Standup notes\n#work/acme", t(2)).unwrap();
        row(&mut store.log, b, 0, Some(5), edit.unwrap().into_bytes());
        doc.set_pinned(true).unwrap().unwrap();
        let snapshot = doc.snapshot().unwrap().into_bytes();
        row(&mut store.log, b, 1, Some(6), snapshot);
        store.notes.push(doc.note().unwrap());

        // From another device: its key arrived with its data.
        let (c, doc, first) = create(&mut store, 3, "Idea from the laptop", false, None);
        row(&mut store.log, c, 0, Some(7), first);
        store.notes.push(doc.note().unwrap());

        // Never acknowledged: key pending, every entry in the outbox.
        let (d, mut doc, first) = create(&mut store, 4, "Offline draft", true, None);
        row(&mut store.log, d, 0, None, first);
        let edit = doc.set_archived(true).unwrap();
        row(&mut store.log, d, 0, None, edit.unwrap().into_bytes());
        store.notes.push(doc.note().unwrap());

        store
    }

    /// Creates a store at `path` with layout `version` and writes `old` into
    /// it the way the sync-era code did, plus a sync cursor, a setting, and
    /// (from version 2) a recovery entry.
    fn seed_old_layout(path: &Path, version: usize, old: &OldStore) {
        let conn = Connection::open(path).unwrap();
        let mut s =
            SqliteStorage::open_with(conn, vault(), &schema::MIGRATIONS[..version]).unwrap();
        let tx = s.conn.transaction().unwrap();
        for n in &old.note_rows {
            tx.execute(
                "INSERT INTO notes (note_id, wrapped_key, key_pending, base)
                 VALUES (?1, ?2, ?3, ?4)",
                params![n.id.to_bytes(), n.wrapped_key, n.key_pending, n.base],
            )
            .unwrap();
        }
        for (i, r) in old.log.iter().enumerate() {
            tx.execute(
                "INSERT INTO log (pos, note_id, update_id, kind, seq, is_pending, bytes)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    OldStore::pos(i),
                    r.note.to_bytes(),
                    r.update_id,
                    r.kind,
                    r.seq,
                    r.seq.is_none(),
                    r.bytes
                ],
            )
            .unwrap();
        }
        tx.execute_batch(
            "UPDATE sqlite_sequence SET seq = 500 WHERE name = 'log';
             INSERT INTO meta (key, value) VALUES ('cursor', 7);
             INSERT INTO settings (key, value) VALUES ('editor.vim', 'true');",
        )
        .unwrap();
        tx.commit().unwrap();
        if version >= 2 {
            let entry =
                RecoveryEntry::create(vault(), &old.key(), &RecoveryKey::generate().unwrap())
                    .unwrap();
            s.set_recovery_entry(&entry).unwrap();
        }
    }

    fn upgrade_from(version: usize) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.sqlite3");
        let old = old_store();
        seed_old_layout(&path, version, &old);

        let s = SqliteStorage::open(&path, vault()).unwrap();
        assert_eq!(
            schema::version(&s.conn).unwrap() as usize,
            schema::MIGRATIONS.len()
        );
        assert_eq!(columns(&s.conn, "log"), ["pos", "note_id", "kind", "bytes"]);
        assert_eq!(
            columns(&s.conn, "notes"),
            ["note_id", "wrapped_key", "base"]
        );
        let cursor: Option<i64> = s
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'cursor'", [], |r| {
                r.get(0)
            })
            .optional()
            .unwrap();
        assert_eq!(cursor, None);
        assert_eq!(s.setting("editor.vim").unwrap().as_deref(), Some("true"));
        assert_eq!(s.recovery_entry().unwrap().is_some(), version >= 2);

        // Every note and every log entry, acknowledged or not, in order and
        // at its old position.
        let ids: Vec<NoteId> = old.note_rows.iter().map(|n| n.id).collect();
        assert_eq!(s.list_notes().unwrap(), ids);
        for n in &old.note_rows {
            let stored = s.load_note(n.id).unwrap().unwrap();
            assert_eq!(stored.wrapped_key, n.wrapped_key);
            assert_eq!(stored.base, n.base);
            let want: Vec<(u64, EntryKind, &[u8])> = old
                .log
                .iter()
                .enumerate()
                .filter(|(_, r)| r.note == n.id)
                .map(|(i, r)| {
                    (
                        OldStore::pos(i),
                        kind_from_sql(r.kind).unwrap(),
                        &r.bytes[..],
                    )
                })
                .collect();
            let got: Vec<(u64, EntryKind, &[u8])> = stored
                .entries
                .iter()
                .map(|e| (e.pos.0, e.kind, &e.bytes[..]))
                .collect();
            assert_eq!(got, want);
        }

        // Every note decodes to what it was.
        let mut replica = Replica::open(s, old.key()).unwrap();
        assert_eq!(replica.notes(), old.notes);

        // New entries go after every position handed out before, and the
        // note still edits and reloads.
        replica
            .set_body(
                note(4),
                "Offline draft, edited",
                Timestamp(1_800_000_000_000),
            )
            .unwrap();
        let s = replica.storage().unwrap();
        let last = s
            .load_note(note(4))
            .unwrap()
            .unwrap()
            .entries
            .last()
            .unwrap()
            .pos;
        assert_eq!(last, LogPos(501));
        drop(replica);
        let s = SqliteStorage::open(&path, vault()).unwrap();
        let replica = Replica::open(s, old.key()).unwrap();
        let edited = replica.note(note(4)).unwrap();
        assert_eq!(edited.body(), "Offline draft, edited");
        assert!(edited.is_archived());
    }

    #[test]
    fn version_one_store_upgrades_and_keeps_every_entry() {
        upgrade_from(1);
    }

    #[test]
    fn version_two_store_drops_sync_data_and_keeps_every_entry() {
        upgrade_from(2);
    }

    #[test]
    fn older_layout_upgrades_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.sqlite3");
        let mut s = SqliteStorage::open(&path, vault()).unwrap();
        s.create_note(note(1), b"key", local(1)).unwrap();
        s.append_local(note(1), local(2)).unwrap();
        s.import_note(note(2), b"imported", &[9; 8]).unwrap();
        s.set_setting("editor.vim", "true").unwrap();
        let notes = [note(1), note(2)].map(|id| s.load_note(id).unwrap());
        drop(s);

        let next = "CREATE TABLE extra (x INTEGER) STRICT;
                    ALTER TABLE notes ADD COLUMN extra INTEGER;";
        let migrations = [schema::MIGRATIONS, &[next]].concat();
        let conn = Connection::open(&path).unwrap();
        let s = SqliteStorage::open_with(conn, vault(), &migrations).unwrap();
        assert_eq!(
            schema::version(&s.conn).unwrap() as usize,
            schema::MIGRATIONS.len() + 1
        );
        assert!(tables(&s.conn).contains(&"extra".to_owned()));
        assert_eq!(s.list_notes().unwrap(), [note(1), note(2)]);
        assert_eq!([note(1), note(2)].map(|id| s.load_note(id).unwrap()), notes);
        assert_eq!(s.setting("editor.vim").unwrap().as_deref(), Some("true"));
    }

    #[test]
    fn uncommitted_write_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.sqlite3");
        let mut s = SqliteStorage::open(&path, vault()).unwrap();
        let tx = s.conn.transaction().unwrap();
        tx.execute(
            "INSERT INTO notes (note_id, wrapped_key, base) VALUES (?1, x'00', NULL)",
            [note(1).to_bytes()],
        )
        .unwrap();
        insert_local(&tx, note(1), &local(1)).unwrap();
        drop(tx);
        drop(s);

        let s = SqliteStorage::open(&path, vault()).unwrap();
        assert!(s.list_notes().unwrap().is_empty());
    }
}
