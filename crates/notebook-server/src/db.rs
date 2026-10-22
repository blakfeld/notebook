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
