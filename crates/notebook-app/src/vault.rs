//! An open vault's runtime: the SQLite store, the replica over it, and the
//! workspace the commands use. There is no sync engine and no network:
//! every edit is written to the store by the replica before it returns.

use std::path::{Path, PathBuf};

use futures::channel::mpsc;
use notebook_core::crypto::{VaultId, VaultKey};
use notebook_core::replica::{Replica, Shared};
use notebook_core::workspace::{Changed, Workspace};
use notebook_storage_sqlite::SqliteStorage;

use crate::error::{AppError, AppResult};

pub const DATABASE_FILE: &str = "vault.sqlite3";

/// SQLite's companion files, which move with the database. The main file
/// comes last, and an I/O error part way moves everything back, so a failed
/// move never leaves a database at the old path missing its WAL. (A crash
/// between two renames still can; the window is tiny, and a WAL with
/// content only exists after an earlier crash.)
const COMPANIONS: [&str; 4] = ["-wal", "-shm", "-journal", ""];

pub fn database_path(data_dir: &Path) -> PathBuf {
    data_dir.join(DATABASE_FILE)
}

pub fn open_storage(path: &Path, vault: VaultId) -> AppResult<SqliteStorage> {
    Ok(SqliteStorage::open(path, vault)?)
}

/// Opens the replica over `storage` and the workspace over it. Returns the
/// workspace and its change stream.
pub fn open(
    storage: SqliteStorage,
    vault_key: VaultKey,
) -> AppResult<(Workspace<SqliteStorage>, mpsc::UnboundedReceiver<Changed>)> {
    let replica = Shared::new(Replica::open(storage, vault_key)?);
    Ok(Workspace::open(replica))
}

/// Moves the database at `path` (with its companion files) aside to
/// `<name>.legacy-<stamp>`, never deleting it. Returns the new path, or
/// `None` when there was nothing to move. On failure the files already
/// moved are moved back. The store must not be open.
pub fn set_aside(path: &Path, stamp: i64) -> AppResult<Option<PathBuf>> {
    let target = with_suffix(path, &format!(".legacy-{stamp}"));
    Ok(move_database(path, &target)?.then_some(target))
}

/// `path` with `suffix` appended to its file name.
pub fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Renames the database at `from` (with its companion files) to `to`.
/// Returns false when there was nothing to move. On failure the files
/// already moved are moved back. Neither store may be open, and nothing may
/// be at `to`.
pub fn move_database(from: &Path, to: &Path) -> AppResult<bool> {
    if !from.exists() {
        return Ok(false);
    }
    let mut moved = Vec::new();
    for suffix in COMPANIONS {
        let (from, to) = (with_suffix(from, suffix), with_suffix(to, suffix));
        match std::fs::rename(&from, &to) {
            Ok(()) => moved.push((from, to)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                for (from, to) in moved.into_iter().rev() {
                    let _ = std::fs::rename(to, from);
                }
                return Err(AppError::internal(err));
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use notebook_core::replica::{Replica, Shared};

    use super::*;
    use crate::state::now;

    /// A database whose last writes are still only in its WAL moves with
    /// it, and the moved copy reads them back.
    #[test]
    fn set_aside_moves_the_wal_with_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = database_path(dir.path());
        let vault = VaultId::from_parts(3, 3);
        let key = VaultKey::generate().unwrap();
        let copy = VaultKey::from_secret_bytes(&*key.to_secret_bytes()).unwrap();
        let replica = Shared::new(Replica::open(open_storage(&path, vault).unwrap(), key).unwrap());
        let id = replica
            .lock()
            .create_note("kept in the WAL", now())
            .unwrap();
        // Never closed, as after a crash: nothing is checkpointed.
        std::mem::forget(replica);
        let mut wal = path.as_os_str().to_owned();
        wal.push("-wal");
        assert!(PathBuf::from(&wal).exists());

        let moved = set_aside(&path, 42).unwrap().unwrap();
        assert!(!path.exists());
        assert!(!PathBuf::from(&wal).exists());
        let replica = Replica::open(open_storage(&moved, vault).unwrap(), copy).unwrap();
        assert_eq!(replica.note(id).unwrap().body(), "kept in the WAL");

        assert_eq!(set_aside(&path, 43).unwrap(), None);
    }
}
