//! SQL for users, vaults, and key bundles. Sessions are in
//! [`super::sessions`].

use notebook_core::auth::SecureRng;
use rand_core::RngCore;
use sqlx::{SqliteConnection, SqlitePool};
use ulid::Ulid;

/// A new server-generated user ID (ULID bytes).
pub fn new_user_id(now: i64) -> [u8; 16] {
    let mut random = [0u8; 16];
    SecureRng.fill_bytes(&mut random);
    let now_ms = u64::try_from(now).unwrap_or(0).saturating_mul(1000);
    Ulid::from_parts(now_ms, u128::from_le_bytes(random)).to_bytes()
}

/// Everything needed to create an account. Used by HTTP registration and the
/// `user add` command, so the two can't drift.
pub struct NewAccount<'a> {
    pub user_id: [u8; 16],
    /// Already normalized.
    pub username: &'a str,
    pub opaque_record: &'a [u8],
    /// SHA-256 of the client's recovery verifier.
    pub recovery_verifier_hash: [u8; 32],
    pub is_admin: bool,
    pub vault_id: [u8; 16],
    pub key_bundle: &'a [u8],
    pub now: i64,
}

#[derive(Debug)]
pub enum CreateError {
    UsernameTaken,
    /// The bundle's vault ID belongs to another vault.
    VaultTaken,
    Db(sqlx::Error),
}

impl From<sqlx::Error> for CreateError {
    fn from(e: sqlx::Error) -> Self {
        match e.as_database_error() {
            Some(db) if db.is_unique_violation() => {
                if db.message().contains("users.username") {
                    Self::UsernameTaken
                } else {
                    Self::VaultTaken
                }
            }
            _ => Self::Db(e),
        }
    }
}

/// Inserts the user, vault (with a storage quota of `quota_bytes` and no
/// usage), and key bundle. Run inside a transaction so a failure creates
/// nothing; the username's `UNIQUE` constraint settles races.
pub async fn create_account(
    tx: &mut SqliteConnection,
    account: &NewAccount<'_>,
    quota_bytes: i64,
) -> Result<(), CreateError> {
    sqlx::query(
        "INSERT INTO users (user_id, username, opaque_record, recovery_verifier_hash, \
         is_admin, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(account.user_id.as_slice())
    .bind(account.username)
    .bind(account.opaque_record)
    .bind(account.recovery_verifier_hash.as_slice())
    .bind(account.is_admin)
    .bind(account.now)
    .bind(account.now)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO vaults (vault_id, user_id, created_at, quota_bytes) VALUES (?, ?, ?, ?)",
    )
    .bind(account.vault_id.as_slice())
    .bind(account.user_id.as_slice())
    .bind(account.now)
    .bind(quota_bytes)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO key_bundles (vault_id, bundle, updated_at) VALUES (?, ?, ?)")
        .bind(account.vault_id.as_slice())
        .bind(account.key_bundle)
        .bind(account.now)
        .execute(&mut *tx)
        .await?;
    Ok(())
}

/// A user with their vault.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct User {
    pub user_id: Vec<u8>,
    pub username: String,
    pub opaque_record: Vec<u8>,
    pub recovery_verifier_hash: Vec<u8>,
    pub is_admin: bool,
    pub vault_id: Vec<u8>,
}

/// Looks up a user by normalized username.
pub async fn find_by_username(
    conn: &mut SqliteConnection,
    username: &str,
) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as(
        "SELECT u.user_id, u.username, u.opaque_record, u.recovery_verifier_hash, \
         u.is_admin, v.vault_id FROM users u JOIN vaults v ON v.user_id = u.user_id \
         WHERE u.username = ?",
    )
    .bind(username)
    .fetch_optional(conn)
    .await
}

pub async fn find_by_id(
    conn: &mut SqliteConnection,
    user_id: &[u8],
) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as(
        "SELECT u.user_id, u.username, u.opaque_record, u.recovery_verifier_hash, \
         u.is_admin, v.vault_id FROM users u JOIN vaults v ON v.user_id = u.user_id \
         WHERE u.user_id = ?",
    )
    .bind(user_id)
    .fetch_optional(conn)
    .await
}

pub async fn key_bundle(
    conn: &mut SqliteConnection,
    vault_id: &[u8],
) -> Result<Option<Vec<u8>>, sqlx::Error> {
    sqlx::query_scalar("SELECT bundle FROM key_bundles WHERE vault_id = ?")
        .bind(vault_id)
        .fetch_optional(conn)
        .await
}

/// Replaces the OPAQUE record and key bundle together, after a passphrase
/// change or recovery. Run inside the transaction that also revokes sessions.
pub async fn replace_credentials(
    tx: &mut SqliteConnection,
    user_id: &[u8],
    vault_id: &[u8],
    opaque_record: &[u8],
    key_bundle: &[u8],
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET opaque_record = ?, updated_at = ? WHERE user_id = ?")
        .bind(opaque_record)
        .bind(now)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE key_bundles SET bundle = ?, updated_at = ? WHERE vault_id = ?")
        .bind(key_bundle)
        .bind(now)
        .bind(vault_id)
        .execute(&mut *tx)
        .await?;
    Ok(())
}

pub async fn has_users(pool: &SqlitePool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users)")
        .fetch_one(pool)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    const QUOTA: i64 = 50 * 1024 * 1024;

    fn account(username: &str, id: u8) -> NewAccount<'_> {
        NewAccount {
            user_id: [id; 16],
            username,
            opaque_record: b"record",
            recovery_verifier_hash: [id; 32],
            is_admin: false,
            vault_id: [id; 16],
            key_bundle: b"bundle",
            now: 1,
        }
    }

    #[tokio::test]
    async fn create_find_and_replace() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::connect(&dir.path().join("db")).await.unwrap();
        db::migrate(&pool).await.unwrap();
        assert!(!has_users(&pool).await.unwrap());

        let mut tx = pool.begin().await.unwrap();
        create_account(&mut tx, &account("alice", 1), QUOTA)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert!(has_users(&pool).await.unwrap());

        let mut conn = pool.acquire().await.unwrap();
        let user = find_by_username(&mut conn, "alice").await.unwrap().unwrap();
        assert_eq!(user.vault_id, vec![1; 16]);
        assert_eq!(
            crate::quota::usage(&mut conn, "alice").await.unwrap(),
            Some(crate::quota::Usage {
                used_bytes: 0,
                quota_bytes: QUOTA
            })
        );
        assert_eq!(user.opaque_record, b"record");
        assert!(find_by_username(&mut conn, "bob").await.unwrap().is_none());
        assert_eq!(
            find_by_id(&mut conn, &[1; 16])
                .await
                .unwrap()
                .unwrap()
                .username,
            "alice"
        );

        replace_credentials(&mut conn, &[1; 16], &[1; 16], b"new", b"newbundle", 2)
            .await
            .unwrap();
        assert_eq!(
            key_bundle(&mut conn, &[1; 16]).await.unwrap().unwrap(),
            b"newbundle"
        );
    }

    #[tokio::test]
    async fn conflicts_are_classified_and_create_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::connect(&dir.path().join("db")).await.unwrap();
        db::migrate(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        create_account(&mut tx, &account("alice", 1), QUOTA)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let mut tx = pool.begin().await.unwrap();
        let err = create_account(&mut tx, &account("alice", 2), QUOTA)
            .await
            .unwrap_err();
        assert!(matches!(err, CreateError::UsernameTaken), "{err:?}");
        drop(tx);

        let mut tx = pool.begin().await.unwrap();
        let mut other = account("bob", 3);
        other.vault_id = [1; 16];
        let err = create_account(&mut tx, &other, QUOTA).await.unwrap_err();
        assert!(matches!(err, CreateError::VaultTaken), "{err:?}");
        drop(tx);

        let mut conn = pool.acquire().await.unwrap();
        assert!(find_by_username(&mut conn, "bob").await.unwrap().is_none());
    }
}
