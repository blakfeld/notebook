//! On-disk layout versions and the migrations between them.

use rusqlite::Connection;

/// Migrations in order; applying entry `i` takes the store from layout
/// version `i` to `i + 1`. Existing entries must never change; add new ones.
pub(crate) const MIGRATIONS: &[&str] = &[V1, V2];

const V1: &str = "
CREATE TABLE meta (key TEXT PRIMARY KEY, value ANY NOT NULL) STRICT;
CREATE TABLE notes (
    note_id BLOB PRIMARY KEY,
    wrapped_key BLOB NOT NULL,
    key_pending INTEGER NOT NULL,
    base BLOB
) STRICT;
CREATE TABLE log (
    pos INTEGER PRIMARY KEY AUTOINCREMENT,
    note_id BLOB NOT NULL REFERENCES notes ON DELETE CASCADE,
    update_id BLOB UNIQUE,
    kind INTEGER NOT NULL,
    seq INTEGER,
    is_pending INTEGER NOT NULL,
    bytes BLOB NOT NULL
) STRICT;
CREATE INDEX log_note ON log(note_id, pos);
CREATE INDEX log_pending ON log(pos) WHERE is_pending = 1;
CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
";

/// The vault's recovery entry, one row at most.
const V2: &str = "
CREATE TABLE vault_meta (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    recovery_entry BLOB NOT NULL
) STRICT;
";

/// The store's layout version.
pub(crate) fn version(conn: &Connection) -> rusqlite::Result<u32> {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
}

/// Applies every migration after `from`, each in its own transaction that
/// also records the new version.
pub(crate) fn migrate(
    conn: &mut Connection,
    from: u32,
    migrations: &[&str],
) -> rusqlite::Result<()> {
    for (i, sql) in migrations.iter().enumerate().skip(from as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i as u32 + 1)?;
        tx.commit()?;
    }
    Ok(())
}
