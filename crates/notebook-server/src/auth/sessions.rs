//! Device sessions: opaque random tokens stored as SHA-256 hashes, rotating
//! refresh tokens with reuse detection, the access-token check, and the
//! [`AuthSession`] extractor for authenticated routes.

use std::time::Duration;

use axum::extract::FromRequestParts;
use axum::http::header;
use axum::http::request::Parts;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use notebook_core::auth::SecureRng;
use notebook_core::auth::api::{SessionInfo, SessionKind, Token, Tokens};
use rand_core::RngCore;
use sha2::{Digest, Sha256};
use sqlx::{SqliteConnection, SqlitePool};
use ulid::Ulid;

use super::error::ApiError;
use super::unix_now;
use crate::app::AppState;

/// Access tokens are valid this long after issue.
pub const ACCESS_TTL_SECS: i64 = 15 * 60;
/// Refresh lifetime for native sessions, renewed on each rotation.
pub const NATIVE_REFRESH_TTL_SECS: i64 = 30 * 24 * 60 * 60;
/// Refresh lifetime for browser sessions, renewed on each rotation.
pub const BROWSER_REFRESH_TTL_SECS: i64 = 12 * 60 * 60;
/// `last_used_at` is written at most this often on access.
const LAST_USED_GRANULARITY_SECS: i64 = 60;
/// How often expired sessions are deleted.
const CLEANUP_INTERVAL: Duration = Duration::from_secs(60 * 60);

const TOKEN_LEN: usize = 32;

/// The identity behind a valid access token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSession {
    pub user_id: Vec<u8>,
    pub vault_id: Vec<u8>,
    pub session_id: Vec<u8>,
}

/// A newly created session's ID and tokens.
#[derive(Debug)]
pub struct Issued {
    pub session_id: [u8; 16],
    pub access_token: Token,
    pub refresh_token: Token,
}

/// A session ID as clients see it: ULID text.
pub fn session_id_text(id: &[u8]) -> String {
    <[u8; 16]>::try_from(id).map_or_else(|_| String::new(), |b| Ulid::from_bytes(b).to_string())
}

pub fn parse_session_id(text: &str) -> Option<[u8; 16]> {
    Ulid::from_string(text).ok().map(|id| id.to_bytes())
}

fn kind_text(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::Native => "native",
        SessionKind::Browser => "browser",
    }
}

fn refresh_ttl(kind: &str) -> i64 {
    if kind == "browser" {
        BROWSER_REFRESH_TTL_SECS
    } else {
        NATIVE_REFRESH_TTL_SECS
    }
}

/// A new random token and the hash that is stored.
fn new_token() -> (Token, [u8; 32]) {
    let mut bytes = [0u8; TOKEN_LEN];
    SecureRng.fill_bytes(&mut bytes);
    let hash = Sha256::digest(bytes).into();
    (Token(URL_SAFE_NO_PAD.encode(bytes)), hash)
}

/// The stored hash of a presented token, or `None` if it isn't a token.
fn token_hash(token: &str) -> Option<[u8; 32]> {
    let bytes = URL_SAFE_NO_PAD.decode(token).ok()?;
    (bytes.len() == TOKEN_LEN).then(|| Sha256::digest(&bytes).into())
}

/// Creates a session for `user_id`'s vault.
pub async fn issue(
    conn: &mut SqliteConnection,
    user_id: &[u8],
    vault_id: &[u8],
    device_name: &str,
    kind: SessionKind,
    now: i64,
) -> Result<Issued, sqlx::Error> {
    let mut random = [0u8; 16];
    SecureRng.fill_bytes(&mut random);
    let now_ms = u64::try_from(now).unwrap_or(0).saturating_mul(1000);
    let session_id = Ulid::from_parts(now_ms, u128::from_le_bytes(random)).to_bytes();
    let (access_token, access_hash) = new_token();
    let (refresh_token, refresh_hash) = new_token();
    let kind = kind_text(kind);
    sqlx::query(
        "INSERT INTO sessions (session_id, vault_id, user_id, device_name, kind, \
         access_hash, access_expires_at, refresh_hash, refresh_expires_at, \
         created_at, last_used_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(session_id.as_slice())
    .bind(vault_id)
    .bind(user_id)
    .bind(device_name)
    .bind(kind)
    .bind(access_hash.as_slice())
    .bind(now + ACCESS_TTL_SECS)
    .bind(refresh_hash.as_slice())
    .bind(now + refresh_ttl(kind))
    .bind(now)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(Issued {
        session_id,
        access_token,
        refresh_token,
    })
}

/// Exchanges a refresh token for a new pair. The old refresh token and access
/// token stop working. A token that was already exchanged ends its session
/// (reuse detection). Every failure is `session_ended`.
pub async fn refresh(pool: &SqlitePool, token: &str, now: i64) -> Result<Tokens, ApiError> {
    let Some(hash) = token_hash(token) else {
        return Err(ApiError::SESSION_ENDED);
    };
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let row: Option<(Vec<u8>, String, i64)> = sqlx::query_as(
        "SELECT session_id, kind, refresh_expires_at FROM sessions WHERE refresh_hash = ?",
    )
    .bind(hash.as_slice())
    .fetch_optional(&mut *tx)
    .await?;
    let Some((session_id, kind, refresh_expires_at)) = row else {
        let reused = sqlx::query(
            "DELETE FROM sessions WHERE session_id = \
             (SELECT session_id FROM refresh_history WHERE hash = ?)",
        )
        .bind(hash.as_slice())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        if reused.rows_affected() > 0 {
            tracing::debug!("refresh token reuse; session revoked");
        }
        return Err(ApiError::SESSION_ENDED);
    };
    if refresh_expires_at <= now {
        delete(&mut tx, &session_id).await?;
        tx.commit().await?;
        return Err(ApiError::SESSION_ENDED);
    }
    let (access_token, access_hash) = new_token();
    let (refresh_token, refresh_hash) = new_token();
    sqlx::query(
        "UPDATE sessions SET access_hash = ?, access_expires_at = ?, refresh_hash = ?, \
         refresh_expires_at = ?, last_used_at = ? WHERE session_id = ?",
    )
    .bind(access_hash.as_slice())
    .bind(now + ACCESS_TTL_SECS)
    .bind(refresh_hash.as_slice())
    .bind(now + refresh_ttl(&kind))
    .bind(now)
    .bind(&session_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO refresh_history (hash, session_id) VALUES (?, ?)")
        .bind(hash.as_slice())
        .bind(&session_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Tokens {
        access_token,
        access_expires_in: ACCESS_TTL_SECS as u64,
        refresh_token,
    })
}

/// `user_id, vault_id, session_id, last_used_at` of a live session.
type LiveSession = (Vec<u8>, Vec<u8>, Vec<u8>, i64);

/// The one access-token check: a live session whose access token is
/// unexpired, or `invalid_token`. `sync-server` calls this directly for
/// WebSocket first-message auth.
pub async fn authenticate_token(pool: &SqlitePool, token: &str) -> Result<AuthSession, ApiError> {
    authenticate_token_at(pool, token, unix_now()).await
}

pub async fn authenticate_token_at(
    pool: &SqlitePool,
    token: &str,
    now: i64,
) -> Result<AuthSession, ApiError> {
    let hash = token_hash(token).ok_or(ApiError::INVALID_TOKEN)?;
    let row: Option<LiveSession> = sqlx::query_as(
        "SELECT user_id, vault_id, session_id, last_used_at FROM sessions \
         WHERE access_hash = ? AND access_expires_at > ?",
    )
    .bind(hash.as_slice())
    .bind(now)
    .fetch_optional(pool)
    .await?;
    let (user_id, vault_id, session_id, last_used_at) = row.ok_or(ApiError::INVALID_TOKEN)?;
    if now - last_used_at >= LAST_USED_GRANULARITY_SECS {
        sqlx::query("UPDATE sessions SET last_used_at = ? WHERE session_id = ?")
            .bind(now)
            .bind(&session_id)
            .execute(pool)
            .await?;
    }
    Ok(AuthSession {
        user_id,
        vault_id,
        session_id,
    })
}

/// Whether the session still exists (not revoked, logged out, or expired),
/// for re-checking long-lived connections.
pub async fn is_active(pool: &SqlitePool, session_id: &[u8]) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sessions WHERE session_id = ? AND refresh_expires_at > ?)",
    )
    .bind(session_id)
    .bind(unix_now())
    .fetch_one(pool)
    .await
}

/// The user's sessions, oldest first, marking `current`.
pub async fn list(
    pool: &SqlitePool,
    user_id: &[u8],
    current: &[u8],
) -> Result<Vec<SessionInfo>, sqlx::Error> {
    let rows: Vec<(Vec<u8>, String, String, i64, i64)> = sqlx::query_as(
        "SELECT session_id, device_name, kind, created_at, last_used_at FROM sessions \
         WHERE user_id = ? ORDER BY created_at, session_id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, device_name, kind, created_at, last_used_at)| SessionInfo {
                current: id == current,
                id: session_id_text(&id),
                device_name,
                session_kind: if kind == "browser" {
                    SessionKind::Browser
                } else {
                    SessionKind::Native
                },
                created_at,
                last_used_at,
            },
        )
        .collect())
}

/// Deletes `session_id` only if it belongs to `user_id`; false otherwise.
pub async fn revoke(
    pool: &SqlitePool,
    user_id: &[u8],
    session_id: &[u8],
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query("DELETE FROM sessions WHERE session_id = ? AND user_id = ?")
        .bind(session_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn delete(conn: &mut SqliteConnection, session_id: &[u8]) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE session_id = ?")
        .bind(session_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Deletes all of the user's sessions except `keep`.
pub async fn delete_all_for_user(
    conn: &mut SqliteConnection,
    user_id: &[u8],
    keep: Option<&[u8]>,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE user_id = ? AND session_id IS NOT ?")
        .bind(user_id)
        .bind(keep)
        .execute(conn)
        .await?;
    Ok(())
}

/// Deletes sessions whose refresh token has expired; returns how many.
pub async fn cleanup_expired(pool: &SqlitePool, now: i64) -> Result<u64, sqlx::Error> {
    let done = sqlx::query("DELETE FROM sessions WHERE refresh_expires_at < ?")
        .bind(now)
        .execute(pool)
        .await?;
    Ok(done.rows_affected())
}

/// Runs [`cleanup_expired`] hourly until the pool closes.
pub fn spawn_cleanup_task(pool: SqlitePool) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(CLEANUP_INTERVAL);
        loop {
            interval.tick().await;
            if pool.is_closed() {
                break;
            }
            match cleanup_expired(&pool, unix_now()).await {
                Ok(0) => {}
                Ok(n) => tracing::debug!("deleted {n} expired sessions"),
                Err(e) => tracing::warn!("expired session cleanup failed: {e}"),
            }
        }
    })
}

/// Requires `Authorization: Bearer <access token>`; anything else is
/// `401 invalid_token`.
impl FromRequestParts<AppState> for AuthSession {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
            .map(|(_, token)| token.trim())
            .ok_or(ApiError::INVALID_TOKEN)?;
        authenticate_token_at(&state.pool, token, state.auth.now()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    struct Fixture {
        _dir: tempfile::TempDir,
        pool: SqlitePool,
    }

    const USER: [u8; 16] = [1; 16];
    const VAULT: [u8; 16] = [2; 16];
    const T0: i64 = 1_700_000_000;

    async fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::connect(&dir.path().join("db")).await.unwrap();
        db::migrate(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        super::super::store::create_account(
            &mut tx,
            &super::super::store::NewAccount {
                user_id: USER,
                username: "alice",
                opaque_record: b"r",
                recovery_verifier_hash: [0; 32],
                is_admin: false,
                vault_id: VAULT,
                key_bundle: b"b",
                now: T0,
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        Fixture { _dir: dir, pool }
    }

    async fn issue_at(f: &Fixture, kind: SessionKind, now: i64) -> Issued {
        let mut conn = f.pool.acquire().await.unwrap();
        issue(&mut conn, &USER, &VAULT, "laptop", kind, now)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn issued_access_token_authenticates() {
        let f = fixture().await;
        let issued = issue_at(&f, SessionKind::Native, T0).await;
        let session = authenticate_token_at(&f.pool, &issued.access_token.0, T0 + 1)
            .await
            .unwrap();
        assert_eq!(session.user_id, USER);
        assert_eq!(session.vault_id, VAULT);
        assert_eq!(session.session_id, issued.session_id);
    }

    #[tokio::test]
    async fn malformed_expired_and_revoked_tokens_are_invalid() {
        let f = fixture().await;
        let issued = issue_at(&f, SessionKind::Native, T0).await;
        let token = issued.access_token.0.clone();
        for bad in [
            "",
            "not base64!",
            "AAAA",
            &URL_SAFE_NO_PAD.encode([0u8; 32]),
        ] {
            assert_eq!(
                authenticate_token_at(&f.pool, bad, T0).await.unwrap_err(),
                ApiError::INVALID_TOKEN,
                "{bad:?}"
            );
        }
        assert_eq!(
            authenticate_token_at(&f.pool, &token, T0 + ACCESS_TTL_SECS + 1)
                .await
                .unwrap_err(),
            ApiError::INVALID_TOKEN
        );
        assert!(revoke(&f.pool, &USER, &issued.session_id).await.unwrap());
        assert_eq!(
            authenticate_token_at(&f.pool, &token, T0)
                .await
                .unwrap_err(),
            ApiError::INVALID_TOKEN
        );
    }

    #[tokio::test]
    async fn refresh_rotates_both_tokens() {
        let f = fixture().await;
        let issued = issue_at(&f, SessionKind::Native, T0).await;
        let tokens = refresh(&f.pool, &issued.refresh_token.0, T0 + 10)
            .await
            .unwrap();
        assert_ne!(tokens.access_token, issued.access_token);
        assert_ne!(tokens.refresh_token, issued.refresh_token);
        assert!(
            authenticate_token_at(&f.pool, &tokens.access_token.0, T0 + 11)
                .await
                .is_ok()
        );
        assert!(
            authenticate_token_at(&f.pool, &issued.access_token.0, T0 + 11)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn reuse_revokes_the_session() {
        let f = fixture().await;
        let issued = issue_at(&f, SessionKind::Native, T0).await;
        let tokens = refresh(&f.pool, &issued.refresh_token.0, T0 + 10)
            .await
            .unwrap();
        assert_eq!(
            refresh(&f.pool, &issued.refresh_token.0, T0 + 20)
                .await
                .unwrap_err(),
            ApiError::SESSION_ENDED
        );
        assert!(
            authenticate_token_at(&f.pool, &tokens.access_token.0, T0 + 21)
                .await
                .is_err()
        );
        assert_eq!(
            refresh(&f.pool, &tokens.refresh_token.0, T0 + 22)
                .await
                .unwrap_err(),
            ApiError::SESSION_ENDED
        );
        assert!(!is_active(&f.pool, &issued.session_id).await.unwrap());
    }

    #[tokio::test]
    async fn reuse_of_any_earlier_token_revokes_the_session() {
        let f = fixture().await;
        let r0 = issue_at(&f, SessionKind::Native, T0).await.refresh_token;
        // The thief rotates twice; the victim then presents the original.
        let r1 = refresh(&f.pool, &r0.0, T0 + 1).await.unwrap();
        let r2 = refresh(&f.pool, &r1.refresh_token.0, T0 + 2).await.unwrap();
        assert_eq!(
            refresh(&f.pool, &r0.0, T0 + 3).await.unwrap_err(),
            ApiError::SESSION_ENDED
        );
        assert_eq!(
            refresh(&f.pool, &r2.refresh_token.0, T0 + 4)
                .await
                .unwrap_err(),
            ApiError::SESSION_ENDED
        );
        let history: i64 = sqlx::query_scalar("SELECT count(*) FROM refresh_history")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(history, 0, "history goes with the session");
    }

    #[tokio::test]
    async fn refresh_lifetimes_depend_on_kind() {
        let f = fixture().await;
        let native = issue_at(&f, SessionKind::Native, T0).await;
        let browser = issue_at(&f, SessionKind::Browser, T0).await;
        let hours = |h: i64| T0 + h * 3600;

        assert_eq!(
            refresh(&f.pool, &browser.refresh_token.0, hours(12) + 1)
                .await
                .unwrap_err(),
            ApiError::SESSION_ENDED
        );
        let renewed = refresh(&f.pool, &native.refresh_token.0, hours(13))
            .await
            .unwrap();

        // Renewed for 30 days from the rotation, not from the first issue.
        let late = hours(13) + NATIVE_REFRESH_TTL_SECS - 1;
        let renewed = refresh(&f.pool, &renewed.refresh_token.0, late)
            .await
            .unwrap();
        assert_eq!(
            refresh(
                &f.pool,
                &renewed.refresh_token.0,
                late + NATIVE_REFRESH_TTL_SECS + 1
            )
            .await
            .unwrap_err(),
            ApiError::SESSION_ENDED
        );
    }

    #[tokio::test]
    async fn stored_rows_hold_no_raw_tokens() {
        let f = fixture().await;
        let issued = issue_at(&f, SessionKind::Native, T0).await;
        let tokens = refresh(&f.pool, &issued.refresh_token.0, T0 + 1)
            .await
            .unwrap();
        type Hashes = (Vec<u8>, Vec<u8>, Option<Vec<u8>>);
        let rows: Vec<Hashes> = sqlx::query_as(
            "SELECT access_hash, refresh_hash, \
             (SELECT hash FROM refresh_history h WHERE h.session_id = s.session_id) \
             FROM sessions s",
        )
        .fetch_all(&f.pool)
        .await
        .unwrap();
        let raw: Vec<Vec<u8>> = [
            &issued.access_token,
            &issued.refresh_token,
            &tokens.access_token,
            &tokens.refresh_token,
        ]
        .iter()
        .map(|t| URL_SAFE_NO_PAD.decode(&t.0).unwrap())
        .collect();
        for (a, r, p) in rows {
            for column in [Some(a), Some(r), p].into_iter().flatten() {
                assert_eq!(column.len(), 32);
                assert!(!raw.contains(&column));
            }
        }
    }

    #[tokio::test]
    async fn last_used_is_throttled() {
        let f = fixture().await;
        let issued = issue_at(&f, SessionKind::Native, T0).await;
        let last_used = || async {
            sqlx::query_scalar::<_, i64>("SELECT last_used_at FROM sessions")
                .fetch_one(&f.pool)
                .await
                .unwrap()
        };
        authenticate_token_at(&f.pool, &issued.access_token.0, T0 + 30)
            .await
            .unwrap();
        assert_eq!(last_used().await, T0);
        authenticate_token_at(&f.pool, &issued.access_token.0, T0 + 61)
            .await
            .unwrap();
        assert_eq!(last_used().await, T0 + 61);
    }

    #[tokio::test]
    async fn cleanup_deletes_expired_sessions() {
        let f = fixture().await;
        let old = issue_at(&f, SessionKind::Browser, T0).await;
        let fresh = issue_at(&f, SessionKind::Native, T0).await;
        let deleted = cleanup_expired(&f.pool, T0 + BROWSER_REFRESH_TTL_SECS + 3600)
            .await
            .unwrap();
        assert_eq!(deleted, 1);
        let ids: Vec<Vec<u8>> = sqlx::query_scalar("SELECT session_id FROM sessions")
            .fetch_all(&f.pool)
            .await
            .unwrap();
        assert_eq!(ids, vec![fresh.session_id.to_vec()]);
        assert!(!ids.contains(&old.session_id.to_vec()));
    }

    #[tokio::test]
    async fn revoke_requires_the_owner() {
        let f = fixture().await;
        let issued = issue_at(&f, SessionKind::Native, T0).await;
        assert!(!revoke(&f.pool, &[9; 16], &issued.session_id).await.unwrap());
        assert!(
            authenticate_token_at(&f.pool, &issued.access_token.0, T0)
                .await
                .is_ok()
        );
    }

    #[test]
    fn session_ids_round_trip_as_text() {
        let id = [7u8; 16];
        assert_eq!(parse_session_id(&session_id_text(&id)), Some(id));
        assert_eq!(parse_session_id("nope"), None);
    }
}
