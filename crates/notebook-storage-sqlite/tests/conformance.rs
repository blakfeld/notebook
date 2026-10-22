//! The shared storage conformance suite, against SQLite.

use notebook_core::crypto::VaultId;
use notebook_core::storage::conformance;
use notebook_storage_sqlite::SqliteStorage;

fn vault() -> VaultId {
    VaultId::from_parts(1_000, 1)
}

#[test]
fn in_memory() {
    conformance::run_all(|| SqliteStorage::open_in_memory(vault()).unwrap());
}

#[test]
fn on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let mut n = 0;
    conformance::run_all(|| {
        n += 1;
        SqliteStorage::open(dir.path().join(format!("{n}.sqlite3")), vault()).unwrap()
    });
}
