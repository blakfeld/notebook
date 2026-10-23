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
//!   [`StorageError::VaultMismatch`] until it is [cleared](Storage::clear).
//! - **Settings are unencrypted.** Note content and keys are stored only as
//!   ciphertext and wrapped keys, but settings are plain text and survive
//!   `clear`, so they must hold only non-sensitive values. Note IDs, the
//!   vault ID, sequence numbers, and sizes are readable from the file.
//! - **Corruption is never repaired.** A file that isn't a valid store fails
//!   with [`StorageError::Corrupt`] and is left untouched, because its outbox
//!   may hold unsynced edits. Resetting is the app's call;
//!   [`SqliteStorage::delete_database`] removes the files.
//! - **Recovery entry.** The vault's [`RecoveryEntry`] is kept in the file
//!   ([`SqliteStorage::recovery_entry`]). It opens only with the recovery
//!   key, so it isn't secret on its own. [`clear`](Storage::clear) removes it
//!   with the vault binding.
//! - **Durability.** Writes use WAL with `synchronous=FULL`: once a call
//!   returns, its change survives a crash or power loss. Each call is one
//!   transaction.

#![forbid(unsafe_code)]

use std::io;
use std::path::Path;

use notebook_core::crypto::{RecoveryEntry, VaultId};
use notebook_core::note::NoteId;
use notebook_core::storage::{
    EntryKind, LocalUpdate, LogEntry, LogPos, OutboxEntry, RemoteBatch, Result, ServerSeq, Storage,
    StorageError, StoredNote, UpdateId,
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

fn kind_to_sql(kind: EntryKind) -> u8 {
    match kind {
        EntryKind::Update => KIND_UPDATE,
        EntryKind::Snapshot => KIND_SNAPSHOT,
    }
}

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

fn update_exists(tx: &Transaction, id: UpdateId) -> Result<bool> {
    tx.query_row("SELECT 1 FROM log WHERE update_id = ?1", [id.0], |_| Ok(()))
        .optional()
        .db()
        .map(|r| r.is_some())
}

fn insert_local(tx: &Transaction, id: NoteId, update: &LocalUpdate) -> Result<()> {
    if update_exists(tx, update.id)? {
        return Err(StorageError::DuplicateUpdate);
    }
    tx.execute(
        "INSERT INTO log (note_id, update_id, kind, seq, is_pending, bytes)
         VALUES (?1, ?2, ?3, NULL, 1, ?4)",
        params![id.to_bytes(), update.id.0, KIND_UPDATE, update.bytes],
    )
    .db()?;
    Ok(())
}

fn read_cursor(conn: &Connection) -> Result<ServerSeq> {
    conn.query_row("SELECT value FROM meta WHERE key = 'cursor'", [], |row| {
        row.get(0)
    })
    .optional()
    .db()
    .map(|v| ServerSeq(v.unwrap_or(0)))
}

/// The body of [`Storage::apply_remote`], run inside `tx`.
fn apply_batch(tx: &Transaction, batch: RemoteBatch) -> Result<()> {
    let current = read_cursor(tx)?;
    if batch.cursor < current {
        return Err(StorageError::CursorRegressed {
            current,
            requested: batch.cursor,
        });
    }
    for (id, wrapped_key) in &batch.keys {
        tx.execute(
            "INSERT OR IGNORE INTO notes (note_id, wrapped_key, key_pending, base)
             VALUES (?1, ?2, 0, NULL)",
            params![id.to_bytes(), wrapped_key],
        )
        .db()?;
    }
    for e in batch.entries {
        if !note_exists(tx, e.note)? {
            return Err(StorageError::NoteNotFound(e.note));
        }
        if let Some(id) = e.id
            && update_exists(tx, id)?
        {
            tx.execute(
                "UPDATE log SET is_pending = 0, seq = ?2 WHERE update_id = ?1 AND is_pending = 1",
                params![id.0, e.seq.0],
            )
            .db()?;
            continue;
        }
        tx.execute(
            "INSERT INTO log (note_id, update_id, kind, seq, is_pending, bytes)
             VALUES (?1, ?2, ?3, ?4, 0, ?5)",
            params![
                e.note.to_bytes(),
                e.id.map(|id| id.0),
                kind_to_sql(e.kind),
                e.seq.0,
                e.bytes
            ],
        )
        .db()?;
    }
    tx.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES ('cursor', ?1)",
        [batch.cursor.0],
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
                "SELECT pos, update_id, kind, seq, bytes FROM log
                 WHERE note_id = ?1 ORDER BY pos",
            )
            .db()?;
        let rows = stmt
            .query_map([id.to_bytes()], |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, Option<[u8; 16]>>(1)?,
                    row.get::<_, u8>(2)?,
                    row.get::<_, Option<u64>>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                ))
            })
            .db()?
            .collect::<rusqlite::Result<Vec<_>>>()
            .db()?;
        let entries = rows
            .into_iter()
            .map(|(pos, update_id, kind, seq, bytes)| {
                Ok(LogEntry {
                    pos: LogPos(pos),
                    id: update_id.map(UpdateId),
                    kind: kind_from_sql(kind)?,
                    seq: seq.map(ServerSeq),
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
            "INSERT INTO notes (note_id, wrapped_key, key_pending, base) VALUES (?1, ?2, 1, NULL)",
            params![id.to_bytes(), wrapped_key],
        )
        .db()?;
        insert_local(&tx, id, &initial)?;
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

    fn outbox(&self) -> Result<Vec<OutboxEntry>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT pos, note_id, update_id, bytes FROM log
                 WHERE is_pending = 1 ORDER BY pos",
            )
            .db()?;
        stmt.query_map([], |row| {
            Ok(OutboxEntry {
                pos: LogPos(row.get(0)?),
                note: NoteId::from_bytes(row.get(1)?),
                id: UpdateId(row.get(2)?),
                bytes: row.get(3)?,
            })
        })
        .db()?
        .collect::<rusqlite::Result<_>>()
        .db()
    }

    fn acknowledge(&mut self, update: UpdateId, seq: ServerSeq) -> Result<()> {
        self.conn
            .execute(
                "UPDATE log SET is_pending = 0, seq = ?2 WHERE update_id = ?1 AND is_pending = 1",
                params![update.0, seq.0],
            )
            .db()?;
        Ok(())
    }

    fn pending_keys(&self) -> Result<Vec<(NoteId, Vec<u8>)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT note_id, wrapped_key FROM notes WHERE key_pending = 1 ORDER BY note_id",
            )
            .db()?;
        stmt.query_map([], |row| Ok((NoteId::from_bytes(row.get(0)?), row.get(1)?)))
            .db()?
            .collect::<rusqlite::Result<_>>()
            .db()
    }

    fn mark_key_uploaded(&mut self, id: NoteId) -> Result<()> {
        self.conn
            .execute(
                "UPDATE notes SET key_pending = 0 WHERE note_id = ?1",
                [id.to_bytes()],
            )
            .db()?;
        Ok(())
    }

    fn cursor(&self) -> Result<ServerSeq> {
        read_cursor(&self.conn)
    }

    fn apply_remote(&mut self, batch: RemoteBatch) -> Result<()> {
        let tx = self.conn.transaction().db()?;
        apply_batch(&tx, batch)?;
        tx.commit().db()
    }

    fn compact(&mut self, id: NoteId, snapshot: &[u8], through: LogPos) -> Result<()> {
        let tx = self.conn.transaction().db()?;
        if !note_exists(&tx, id)? {
            return Err(StorageError::NoteNotFound(id));
        }
        tx.execute(
            "DELETE FROM log WHERE note_id = ?1 AND pos <= ?2 AND is_pending = 0",
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

    fn clear(mut self) -> Result<()> {
        let tx = self.conn.transaction().db()?;
        tx.execute_batch(
            "DELETE FROM log;
             DELETE FROM notes;
             DELETE FROM vault_meta;
             DELETE FROM meta WHERE key IN ('vault_id', 'cursor');",
        )
        .db()?;
        tx.commit().db()?;
        self.conn.close().map_err(|(_, e)| e).db()
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
    use notebook_core::crypto::{RecoveryKey, VaultKey};
    use notebook_core::storage::RemoteEntry;

    use super::*;

    fn vault() -> VaultId {
        VaultId::from_parts(1_000, 1)
    }

    fn note(n: u128) -> NoteId {
        NoteId::from_parts(1_000, n)
    }

    fn local(n: u8) -> LocalUpdate {
        LocalUpdate {
            id: UpdateId([n; 16]),
            bytes: vec![n; 4],
        }
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

    #[test]
    fn fresh_store_has_version_two_layout() {
        let s = SqliteStorage::open_in_memory(vault()).unwrap();
        assert_eq!(schema::version(&s.conn).unwrap(), 2);
        assert_eq!(schema::MIGRATIONS.len(), 2);
        assert_eq!(
            tables(&s.conn),
            ["log", "meta", "notes", "settings", "vault_meta"]
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
    fn clear_removes_the_recovery_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.sqlite3");
        let mut s = SqliteStorage::open(&path, vault()).unwrap();
        s.set_recovery_entry(&recovery_entry().0).unwrap();
        s.clear().unwrap();
        let s = SqliteStorage::open(&path, vault()).unwrap();
        assert_eq!(s.recovery_entry().unwrap(), None);
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
        s.clear().unwrap();
        assert_eq!(SqliteStorage::peek(&path).unwrap(), None);
    }

    /// A version-one store, as a signed-in install left it, upgrades with
    /// every note, outbox entry, cursor, and setting intact and an empty
    /// recovery entry slot.
    #[test]
    fn version_one_store_upgrades_to_version_two() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.sqlite3");
        let conn = Connection::open(&path).unwrap();
        let mut s = SqliteStorage::open_with(conn, vault(), &schema::MIGRATIONS[..1]).unwrap();
        assert_eq!(schema::version(&s.conn).unwrap(), 1);
        s.create_note(note(1), b"key", local(1)).unwrap();
        s.append_local(note(1), local(2)).unwrap();
        s.acknowledge(UpdateId([1; 16]), ServerSeq(3)).unwrap();
        s.apply_remote(RemoteBatch {
            keys: vec![],
            entries: vec![],
            cursor: ServerSeq(3),
        })
        .unwrap();
        s.set_setting("editor.vim", "true").unwrap();
        let loaded = s.load_note(note(1)).unwrap();
        let outbox = s.outbox().unwrap();
        drop(s);

        let mut s = SqliteStorage::open(&path, vault()).unwrap();
        assert_eq!(schema::version(&s.conn).unwrap(), 2);
        assert_eq!(s.list_notes().unwrap(), [note(1)]);
        assert_eq!(s.load_note(note(1)).unwrap(), loaded);
        assert_eq!(s.outbox().unwrap(), outbox);
        assert_eq!(s.cursor().unwrap(), ServerSeq(3));
        assert_eq!(s.setting("editor.vim").unwrap().as_deref(), Some("true"));
        assert_eq!(s.recovery_entry().unwrap(), None);
        s.set_recovery_entry(&recovery_entry().0).unwrap();
        assert!(s.recovery_entry().unwrap().is_some());
    }

    #[test]
    fn older_layout_upgrades_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.sqlite3");
        let mut s = SqliteStorage::open(&path, vault()).unwrap();
        s.create_note(note(1), b"key", local(1)).unwrap();
        s.append_local(note(1), local(2)).unwrap();
        s.acknowledge(UpdateId([1; 16]), ServerSeq(3)).unwrap();
        s.apply_remote(RemoteBatch {
            keys: vec![(note(2), b"remote".to_vec())],
            entries: vec![RemoteEntry {
                note: note(2),
                kind: EntryKind::Snapshot,
                id: None,
                seq: ServerSeq(4),
                bytes: vec![9; 8],
            }],
            cursor: ServerSeq(4),
        })
        .unwrap();
        s.set_setting("editor.vim", "true").unwrap();
        let notes = [note(1), note(2)].map(|id| s.load_note(id).unwrap());
        let outbox = s.outbox().unwrap();
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
        assert_eq!(s.outbox().unwrap(), outbox);
        assert_eq!(s.cursor().unwrap(), ServerSeq(4));
        assert_eq!(s.setting("editor.vim").unwrap().as_deref(), Some("true"));
    }

    #[test]
    fn uncommitted_batch_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.sqlite3");
        let mut s = SqliteStorage::open(&path, vault()).unwrap();
        let tx = s.conn.transaction().unwrap();
        apply_batch(
            &tx,
            RemoteBatch {
                keys: vec![(note(1), b"key".to_vec())],
                entries: vec![RemoteEntry {
                    note: note(1),
                    kind: EntryKind::Update,
                    id: Some(UpdateId([1; 16])),
                    seq: ServerSeq(5),
                    bytes: vec![1],
                }],
                cursor: ServerSeq(5),
            },
        )
        .unwrap();
        drop(tx);
        drop(s);

        let s = SqliteStorage::open(&path, vault()).unwrap();
        assert!(s.list_notes().unwrap().is_empty());
        assert_eq!(s.cursor().unwrap(), ServerSeq(0));
    }
}
