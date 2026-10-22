//! Per-vault storage quotas. A vault's usage is the total payload length of
//! its stored updates and snapshots, kept current by [`reserve`] in the same
//! transaction as every sync write, so checking it is a single-row update.

use sqlx::SqliteConnection;

/// Adds `delta` payload bytes to the vault's usage, inside the caller's
/// write transaction. `false`, changing nothing, if a positive delta would
/// take usage over the quota; non-positive deltas always succeed, so
/// compaction and purge work at or over quota. One conditional statement, so
/// concurrent writes can't both slip under the limit.
pub async fn reserve(
    tx: &mut SqliteConnection,
    vault_id: &[u8],
    delta: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE vaults SET used_bytes = used_bytes + ?1 \
         WHERE vault_id = ?2 AND (?1 <= 0 OR used_bytes + ?1 <= quota_bytes)",
    )
    .bind(delta)
    .bind(vault_id)
    .execute(tx)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// A vault's usage and quota, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::FromRow)]
pub struct Usage {
    pub used_bytes: i64,
    pub quota_bytes: i64,
}

/// The usage of `username`'s vault (normalized), or `None` for no such user.
pub async fn usage(
    conn: &mut SqliteConnection,
    username: &str,
) -> Result<Option<Usage>, sqlx::Error> {
    sqlx::query_as(
        "SELECT v.used_bytes, v.quota_bytes FROM vaults v JOIN users u USING (user_id) \
         WHERE u.username = ?",
    )
    .bind(username)
    .fetch_optional(conn)
    .await
}

/// Sets `username`'s vault quota, which may be below its usage (that only
/// blocks growth). `None`, changing nothing, for no such user.
pub async fn set(
    conn: &mut SqliteConnection,
    username: &str,
    quota_bytes: i64,
) -> Result<Option<Usage>, sqlx::Error> {
    sqlx::query_as(
        "UPDATE vaults SET quota_bytes = ?1 \
         WHERE user_id = (SELECT user_id FROM users WHERE username = ?2) \
         RETURNING used_bytes, quota_bytes",
    )
    .bind(quota_bytes)
    .bind(username)
    .fetch_optional(conn)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[tokio::test]
    async fn reserve_at_below_and_above_quota() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::open(&dir.path().join("db")).await.unwrap();
        sqlx::query(
            "INSERT INTO users (user_id, username, opaque_record, recovery_verifier_hash, \
             created_at, updated_at) VALUES (x'01', 'alice', x'00', x'00', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO vaults (vault_id, user_id, created_at, quota_bytes) \
             VALUES (x'11', x'01', 0, 100)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let vault = [0x11u8];

        assert!(reserve(&mut conn, &vault, 60).await.unwrap());
        assert!(
            reserve(&mut conn, &vault, 40).await.unwrap(),
            "exactly at quota"
        );
        assert!(!reserve(&mut conn, &vault, 1).await.unwrap(), "over quota");
        assert!(reserve(&mut conn, &vault, 0).await.unwrap());
        let at = usage(&mut conn, "alice").await.unwrap().unwrap();
        assert_eq!(
            at,
            Usage {
                used_bytes: 100,
                quota_bytes: 100
            }
        );

        // Lowered below usage: growth is blocked, shrinking is not.
        assert_eq!(
            set(&mut conn, "alice", 50).await.unwrap(),
            Some(Usage {
                used_bytes: 100,
                quota_bytes: 50
            })
        );
        assert!(!reserve(&mut conn, &vault, 1).await.unwrap());
        assert!(reserve(&mut conn, &vault, -30).await.unwrap());
        assert!(!reserve(&mut conn, &vault, 1).await.unwrap(), "still over");
        assert!(reserve(&mut conn, &vault, -30).await.unwrap());
        assert!(reserve(&mut conn, &vault, 10).await.unwrap(), "below again");
        assert_eq!(
            usage(&mut conn, "alice").await.unwrap().unwrap().used_bytes,
            50
        );

        assert_eq!(usage(&mut conn, "bob").await.unwrap(), None);
        assert_eq!(set(&mut conn, "bob", 1).await.unwrap(), None);
    }
}
