//! SQLite pool setup and embedded migrations.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::migrate::{MigrateError, Migrator};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

/// Migrations from `migrations/`, embedded at compile time.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Opens (creating if absent) the database file at `path`.
pub async fn connect(path: &Path) -> Result<SqlitePool, DbError> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        // WAL is required by Litestream and lets readers run during writes.
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    SqlitePoolOptions::new()
        .connect_with(options)
        .await
        .map_err(DbError::Open)
}

/// Applies pending migrations; refuses a database migrated by a newer binary.
pub async fn migrate(pool: &SqlitePool) -> Result<(), DbError> {
    MIGRATOR.run(pool).await.map_err(|e| match e {
        MigrateError::VersionMissing(version) => DbError::SchemaNewer(version),
        e => DbError::Migrate(e),
    })
}

#[derive(Debug)]
pub enum DbError {
    Open(sqlx::Error),
    SchemaNewer(i64),
    Migrate(MigrateError),
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbError::Open(e) => write!(f, "cannot open database: {e}"),
            DbError::SchemaNewer(version) => write!(
                f,
                "database schema is newer than this binary (unknown migration {version}); \
                 run a newer notebook-server or restore the pre-upgrade backup"
            ),
            DbError::Migrate(e) => write!(f, "cannot migrate database: {e}"),
        }
    }
}

impl std::error::Error for DbError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn first_start_creates_and_migrates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.db");
        assert!(!path.exists());
        let pool = connect(&path).await.unwrap();
        migrate(&pool).await.unwrap();
        assert!(path.exists());
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(mode, "wal");
        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(fk, 1);
    }

    #[tokio::test]
    async fn remigration_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let schema = |pool: SqlitePool| async move {
            sqlx::query_scalar::<_, String>(
                "SELECT group_concat(sql, ';') FROM sqlite_master ORDER BY name",
            )
            .fetch_one(&pool)
            .await
            .unwrap_or_default()
        };
        let pool = connect(&path).await.unwrap();
        migrate(&pool).await.unwrap();
        let before = schema(pool.clone()).await;
        pool.close().await;

        let pool = connect(&path).await.unwrap();
        migrate(&pool).await.unwrap();
        assert_eq!(schema(pool).await, before);
    }

    #[tokio::test]
    async fn accounts_tables_exist_and_usernames_are_unique() {
        let dir = tempfile::tempdir().unwrap();
        let pool = connect(&dir.path().join("db")).await.unwrap();
        migrate(&pool).await.unwrap();
        for table in ["users", "vaults", "key_bundles", "sessions", "server_meta"] {
            let n: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
            )
            .bind(table)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(n, 1, "{table}");
        }
        let insert = |id: u8| {
            sqlx::query(
                "INSERT INTO users (user_id, username, opaque_record, recovery_verifier_hash, \
                 created_at, updated_at) VALUES (?, 'alice', x'00', x'00', 0, 0)",
            )
            .bind(vec![id; 16])
            .execute(&pool)
        };
        insert(1).await.unwrap();
        let err = insert(2).await.unwrap_err();
        assert!(err.to_string().contains("UNIQUE"), "{err}");
    }

    #[tokio::test]
    async fn sync_tables_exist_and_op_ids_are_unique_per_vault() {
        let dir = tempfile::tempdir().unwrap();
        let pool = connect(&dir.path().join("db")).await.unwrap();
        migrate(&pool).await.unwrap();
        let columns = |table: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, String>("SELECT name FROM pragma_table_info(?)")
                    .bind(table)
                    .fetch_all(&pool)
                    .await
                    .unwrap()
            }
        };
        assert!(columns("vaults").await.contains(&"sync_head".to_owned()));
        assert_eq!(
            columns("sync_notes").await,
            [
                "vault_id",
                "note_id",
                "wrapped_key",
                "created_seq",
                "purged_seq"
            ]
        );
        assert_eq!(
            columns("ops").await,
            ["vault_id", "seq", "op_id", "note_id", "payload"]
        );
        assert_eq!(
            columns("snapshots").await,
            ["vault_id", "note_id", "covers", "payload"]
        );
        let unique: Vec<(i64, String)> = sqlx::query_as(
            "SELECT i.\"unique\", group_concat(c.name, ',') FROM pragma_index_list('ops') i, \
             pragma_index_info(i.name) c WHERE i.name = 'ops_op_id' GROUP BY i.name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(unique, [(1, "vault_id,op_id".to_owned())]);

        for (user, vault) in [(1u8, 11u8), (2, 12)] {
            sqlx::query(
                "INSERT INTO users (user_id, username, opaque_record, recovery_verifier_hash, \
                 created_at, updated_at) VALUES (?, ?, x'00', x'00', 0, 0)",
            )
            .bind(vec![user; 16])
            .bind(format!("u{user}"))
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO vaults (vault_id, user_id, created_at) VALUES (?, ?, 0)")
                .bind(vec![vault; 16])
                .bind(vec![user; 16])
                .execute(&pool)
                .await
                .unwrap();
        }
        let insert = |vault: u8, seq: i64| {
            sqlx::query(
                "INSERT INTO ops (vault_id, seq, op_id, note_id, payload) \
                 VALUES (?, ?, x'01', x'02', x'03')",
            )
            .bind(vec![vault; 16])
            .bind(seq)
            .execute(&pool)
        };
        insert(11, 1).await.unwrap();
        insert(12, 1).await.unwrap();
        let err = insert(11, 2).await.unwrap_err();
        assert!(err.to_string().contains("UNIQUE"), "{err}");
    }

    #[tokio::test]
    async fn refuses_newer_schema() {
        let dir = tempfile::tempdir().unwrap();
        let pool = connect(&dir.path().join("db")).await.unwrap();
        migrate(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO _sqlx_migrations \
             (version, description, success, checksum, execution_time) \
             VALUES (99999999, 'from the future', TRUE, x'00', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let err = migrate(&pool).await.unwrap_err();
        assert!(matches!(err, DbError::SchemaNewer(99999999)), "{err:?}");
        assert!(err.to_string().contains("schema is newer than this binary"));
    }
}
