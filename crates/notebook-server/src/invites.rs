//! Invite codes, which gate HTTP registration: generation, normalization,
//! storage, and the `invite` operator commands.
//!
//! A code is 100 random bits in Crockford base32, printed as four groups of
//! five. Only the SHA-256 of its normalized form is stored, and codes are
//! never logged. Status (active, used, expired, revoked) is derived from the
//! row's timestamps, so expiry needs no background job.

use std::fmt;
use std::time::Duration;

use notebook_core::auth::SecureRng;
use rand_core::RngCore;
use sha2::{Digest, Sha256};
use sqlx::{SqliteConnection, SqlitePool};
use ulid::Ulid;

use crate::auth::unix_now;
use crate::config::Config;
use crate::db;

/// Crockford's base32 alphabet: no `I`, `L`, `O`, or `U`.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Characters in a code, without separators (20 × 5 = 100 bits).
const CODE_CHARS: usize = 20;

/// Characters per dash-separated group in a printed code.
const GROUP_CHARS: usize = 5;

/// Characters of an invite's ULID shown by `invite list`.
pub const SHORT_ID_CHARS: usize = 8;

/// A new code, such as `7KQ2M-XW9PD-0RT4H-C6NBJ`.
pub fn generate() -> String {
    let mut random = [0u8; 16];
    SecureRng.fill_bytes(&mut random);
    let bits = u128::from_le_bytes(random);
    let mut code = String::with_capacity(CODE_CHARS + CODE_CHARS / GROUP_CHARS);
    for i in 0..CODE_CHARS {
        if i > 0 && i % GROUP_CHARS == 0 {
            code.push('-');
        }
        code.push(char::from(ALPHABET[((bits >> (5 * i)) & 31) as usize]));
    }
    code
}

/// The canonical form of a typed code: trimmed, dashes dropped, upper-cased,
/// with `I`/`L` read as `1` and `O` as `0`. `None` if it can't be a code.
pub fn normalize(input: &str) -> Option<String> {
    let code: String = input
        .trim()
        .chars()
        .filter(|&c| c != '-')
        .map(|c| match c.to_ascii_uppercase() {
            'I' | 'L' => '1',
            'O' => '0',
            c => c,
        })
        .collect();
    (code.len() == CODE_CHARS && code.bytes().all(|b| ALPHABET.contains(&b))).then_some(code)
}

/// What is stored for a code: the SHA-256 of its normalized form.
fn hash(normalized: &str) -> [u8; 32] {
    Sha256::digest(normalized.as_bytes()).into()
}

/// An invite's state, derived from its row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Active,
    Used,
    Expired,
    Revoked,
}

impl Status {
    fn derive(revoked_at: Option<i64>, used_at: Option<i64>, expires_at: i64, now: i64) -> Self {
        if revoked_at.is_some() {
            Self::Revoked
        } else if used_at.is_some() {
            Self::Used
        } else if expires_at <= now {
            Self::Expired
        } else {
            Self::Active
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Active => "active",
            Self::Used => "used",
            Self::Expired => "expired",
            Self::Revoked => "revoked",
        })
    }
}

/// An invite as `invite list` shows it. Never carries the code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    pub id: Ulid,
    pub note: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
    pub status: Status,
    pub used_at: Option<i64>,
    /// The username registered with it, for used invites.
    pub used_by: Option<String>,
}

impl Invite {
    /// The last [`SHORT_ID_CHARS`] characters of the ID, which `revoke`
    /// accepts.
    pub fn short_id(&self) -> String {
        let id = self.id.to_string();
        id[id.len() - SHORT_ID_CHARS..].to_owned()
    }
}

/// Stores a new invite expiring `ttl` after `now`, returning it and its code.
pub async fn create(
    pool: &SqlitePool,
    now: i64,
    ttl: Duration,
    note: Option<&str>,
) -> Result<(Invite, String), sqlx::Error> {
    let code = generate();
    let normalized = normalize(&code).expect("generated codes are well formed");
    let mut random = [0u8; 16];
    SecureRng.fill_bytes(&mut random);
    let now_ms = u64::try_from(now).unwrap_or(0).saturating_mul(1000);
    let id = Ulid::from_parts(now_ms, u128::from_le_bytes(random));
    let expires_at = now.saturating_add(i64::try_from(ttl.as_secs()).unwrap_or(i64::MAX));
    sqlx::query(
        "INSERT INTO invites (id, code_hash, note, created_at, expires_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(id.to_bytes().as_slice())
    .bind(hash(&normalized).as_slice())
    .bind(note)
    .bind(now)
    .bind(expires_at)
    .execute(pool)
    .await?;
    let invite = Invite {
        id,
        note: note.map(str::to_owned),
        created_at: now,
        expires_at,
        status: Status::Active,
        used_at: None,
        used_by: None,
    };
    Ok((invite, code))
}

/// `id, note, created_at, expires_at, revoked_at, used_at, username`.
type InviteRow = (
    Vec<u8>,
    Option<String>,
    i64,
    i64,
    Option<i64>,
    Option<i64>,
    Option<String>,
);

/// Every invite, oldest first. IDs are dated to the second, so insertion
/// order (`rowid`) breaks ties.
async fn all(conn: &mut SqliteConnection, now: i64) -> Result<Vec<Invite>, sqlx::Error> {
    let rows: Vec<InviteRow> = sqlx::query_as(
        "SELECT i.id, i.note, i.created_at, i.expires_at, i.revoked_at, i.used_at, u.username \
         FROM invites i LEFT JOIN users u ON u.user_id = i.used_by \
         ORDER BY i.created_at, i.rowid",
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(
            |(id, note, created_at, expires_at, revoked_at, used_at, used_by)| {
                let id = Ulid::from_bytes(<[u8; 16]>::try_from(id).ok()?);
                Some(Invite {
                    id,
                    note,
                    created_at,
                    expires_at,
                    status: Status::derive(revoked_at, used_at, expires_at, now),
                    used_at,
                    used_by,
                })
            },
        )
        .collect())
}

/// Invites in creation order; only active ones unless `include_all`.
pub async fn list(
    pool: &SqlitePool,
    now: i64,
    include_all: bool,
) -> Result<Vec<Invite>, sqlx::Error> {
    let mut invites = all(&mut *pool.acquire().await?, now).await?;
    invites.retain(|i| include_all || i.status == Status::Active);
    Ok(invites)
}

/// Why an invite could not be revoked.
#[derive(Debug)]
pub enum RevokeError {
    Unknown,
    Ambiguous,
    Used { by: Option<String> },
    AlreadyRevoked,
    Expired,
    Db(sqlx::Error),
}

impl From<sqlx::Error> for RevokeError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e)
    }
}

impl fmt::Display for RevokeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown => f.write_str("no invite has that ID"),
            Self::Ambiguous => {
                f.write_str("more than one invite ends with that ID; give more of it")
            }
            Self::Used { by: Some(by) } => write!(f, "that invite was already used by {by}"),
            Self::Used { by: None } => f.write_str("that invite was already used"),
            Self::AlreadyRevoked => f.write_str("that invite is already revoked"),
            Self::Expired => f.write_str("that invite has already expired"),
            Self::Db(e) => write!(f, "database error: {e}"),
        }
    }
}

impl std::error::Error for RevokeError {}

/// Revokes the active invite whose ID ends with `id` (case-insensitive).
/// Anything else changes nothing.
pub async fn revoke(pool: &SqlitePool, id: &str, now: i64) -> Result<Invite, RevokeError> {
    let suffix = id.trim().to_ascii_uppercase();
    if suffix.is_empty() {
        return Err(RevokeError::Unknown);
    }
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let mut matches = all(&mut tx, now).await?;
    matches.retain(|i| i.id.to_string().ends_with(&suffix));
    let mut invite = match matches.len() {
        0 => return Err(RevokeError::Unknown),
        1 => matches.remove(0),
        _ => return Err(RevokeError::Ambiguous),
    };
    match invite.status {
        Status::Active => {}
        Status::Used => return Err(RevokeError::Used { by: invite.used_by }),
        Status::Revoked => return Err(RevokeError::AlreadyRevoked),
        Status::Expired => return Err(RevokeError::Expired),
    }
    sqlx::query("UPDATE invites SET revoked_at = ? WHERE id = ?")
        .bind(now)
        .bind(invite.id.to_bytes().as_slice())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    invite.status = Status::Revoked;
    Ok(invite)
}

/// True if `code` names an active invite. Every other case (malformed,
/// unknown, expired, revoked, used) is the same `false`.
pub async fn check(conn: &mut SqliteConnection, code: &str, now: i64) -> Result<bool, sqlx::Error> {
    let Some(normalized) = normalize(code) else {
        return Ok(false);
    };
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM invites WHERE code_hash = ? \
         AND used_at IS NULL AND revoked_at IS NULL AND expires_at > ?)",
    )
    .bind(hash(&normalized).as_slice())
    .bind(now)
    .fetch_one(conn)
    .await
}

/// Marks the active invite `code` as used by `user_id`, in the caller's
/// write transaction, so it commits or rolls back with the account. One
/// conditional statement, so of concurrent consumers exactly one wins.
/// `false` if the code isn't active.
pub async fn consume(
    tx: &mut SqliteConnection,
    code: &str,
    user_id: &[u8],
    now: i64,
) -> Result<bool, sqlx::Error> {
    let Some(normalized) = normalize(code) else {
        return Ok(false);
    };
    let result = sqlx::query(
        "UPDATE invites SET used_at = ?1, used_by = ?2 WHERE code_hash = ?3 \
         AND used_at IS NULL AND revoked_at IS NULL AND expires_at > ?1",
    )
    .bind(now)
    .bind(user_id)
    .bind(hash(&normalized).as_slice())
    .execute(tx)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// A Unix time as RFC 3339 in UTC, such as `2027-01-15T08:00:00Z`.
pub fn rfc3339(unix: i64) -> String {
    // Howard Hinnant's days-to-civil algorithm.
    let (days, secs) = (unix.div_euclid(86_400), unix.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

/// One `invite list` line.
pub fn describe(invite: &Invite) -> String {
    let mut line = format!(
        "{}  created {}  expires {}  {}",
        invite.short_id(),
        rfc3339(invite.created_at),
        rfc3339(invite.expires_at),
        invite.status,
    );
    if invite.status == Status::Used {
        line += &format!(
            " by {} at {}",
            invite.used_by.as_deref().unwrap_or("(unknown user)"),
            invite.used_at.map_or_else(String::new, rfc3339),
        );
    }
    if let Some(note) = &invite.note {
        // Debug-quoted, so control characters can't reach the terminal.
        line += &format!("  note {note:?}");
    }
    line
}

/// `invite create`: prints only the code on stdout, and its expiry on stderr.
pub async fn create_command(
    config: Config,
    ttl: Duration,
    note: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let pool = db::open(&config.database_path).await?;
    let result = create(&pool, unix_now(), ttl, note.as_deref()).await;
    pool.close().await;
    let (invite, code) = result?;
    println!("{code}");
    eprintln!(
        "Invite {} expires {}.",
        invite.short_id(),
        rfc3339(invite.expires_at)
    );
    Ok(())
}

/// `invite list [--all]`.
pub async fn list_command(
    config: Config,
    include_all: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let pool = db::open(&config.database_path).await?;
    let result = list(&pool, unix_now(), include_all).await;
    pool.close().await;
    for invite in result? {
        println!("{}", describe(&invite));
    }
    Ok(())
}

/// `invite revoke <id>`.
pub async fn revoke_command(config: Config, id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let pool = db::open(&config.database_path).await?;
    let result = revoke(&pool, id, unix_now()).await;
    pool.close().await;
    let invite = result?;
    println!("Revoked invite {}.", invite.short_id());
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    const NOW: i64 = 1_800_000_000;
    const DAY: Duration = Duration::from_secs(86_400);

    async fn pool() -> (tempfile::TempDir, SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::open(&dir.path().join("db")).await.unwrap();
        (dir, pool)
    }

    async fn add_user(pool: &SqlitePool, id: u8, name: &str) {
        sqlx::query(
            "INSERT INTO users (user_id, username, opaque_record, recovery_verifier_hash, \
             created_at, updated_at) VALUES (?, ?, x'00', x'00', 0, 0)",
        )
        .bind(vec![id; 16])
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn consume_now(pool: &SqlitePool, code: &str, user: u8, now: i64) -> bool {
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        let consumed = consume(&mut tx, code, &[user; 16], now).await.unwrap();
        tx.commit().await.unwrap();
        consumed
    }

    async fn check_now(pool: &SqlitePool, code: &str, now: i64) -> bool {
        check(&mut pool.acquire().await.unwrap(), code, now)
            .await
            .unwrap()
    }

    #[test]
    fn codes_have_the_printed_format() {
        let code = generate();
        let groups: Vec<&str> = code.split('-').collect();
        assert_eq!(groups.len(), 4, "{code}");
        for group in groups {
            assert_eq!(group.len(), 5, "{code}");
            assert!(group.bytes().all(|b| ALPHABET.contains(&b)), "{code}");
        }
        assert_eq!(normalize(&code), Some(code.replace('-', "")));
    }

    #[test]
    fn codes_are_distinct() {
        let codes: HashSet<String> = (0..10_000).map(|_| generate()).collect();
        assert_eq!(codes.len(), 10_000);
    }

    #[test]
    fn normalization_forgives_typing() {
        let canonical = Some("7KQ2MXW9PD0RT4HC6NBJ".to_owned());
        for typed in [
            "7KQ2M-XW9PD-0RT4H-C6NBJ",
            "7kq2m-xw9pd-0rt4h-c6nbj",
            "7kq2mxw9pd0rt4hc6nbj",
            "  7KQ2M-XW9PD-0RT4H-C6NBJ\n",
            "7KQ2M-XW9PD-ORT4H-C6NBJ",
            "7KQ2M--XW9PD0RT4HC6NBJ",
        ] {
            assert_eq!(normalize(typed), canonical, "{typed:?}");
        }
        assert_eq!(
            normalize("IL0O1-00000-00000-00000"),
            Some("11001000000000000000".to_owned())
        );
        for bad in [
            "",
            "7KQ2M-XW9PD-0RT4H",
            "7KQ2M-XW9PD-0RT4H-C6NBJ-X",
            "UKQ2M-XW9PD-0RT4H-C6NBJ",
            "7KQ2M XW9PD 0RT4H C6NBJ",
            "7KQ2M-XW9PD-0RT4H-C6NBÉ",
        ] {
            assert_eq!(normalize(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn rfc3339_utc() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(NOW), "2027-01-15T08:00:00Z");
        assert_eq!(rfc3339(951_782_400 + 86_399), "2000-02-29T23:59:59Z");
    }

    #[tokio::test]
    async fn statuses_and_listing() {
        let (_dir, pool) = pool().await;
        add_user(&pool, 1, "alice").await;
        let (active, _) = create(&pool, NOW, DAY, Some("for bob")).await.unwrap();
        let (used, used_code) = create(&pool, NOW, DAY, None).await.unwrap();
        let (expired, _) = create(&pool, NOW - 2 * 86_400, DAY, None).await.unwrap();
        let (revoked, _) = create(&pool, NOW, DAY, None).await.unwrap();
        assert!(consume_now(&pool, &used_code, 1, NOW + 5).await);
        revoke(&pool, &revoked.short_id(), NOW).await.unwrap();

        let shown = list(&pool, NOW + 10, false).await.unwrap();
        assert_eq!(shown, std::slice::from_ref(&active));
        assert_eq!(shown[0].note.as_deref(), Some("for bob"));
        assert_eq!(shown[0].expires_at, NOW + 86_400);

        let every = list(&pool, NOW + 10, true).await.unwrap();
        let status = |id: Ulid| every.iter().find(|i| i.id == id).unwrap().clone();
        assert_eq!(status(active.id).status, Status::Active);
        assert_eq!(status(expired.id).status, Status::Expired);
        assert_eq!(status(revoked.id).status, Status::Revoked);
        let used = status(used.id);
        assert_eq!(used.status, Status::Used);
        assert_eq!(used.used_by.as_deref(), Some("alice"));
        assert_eq!(used.used_at, Some(NOW + 5));
        assert!(describe(&used).ends_with(" used by alice at 2027-01-15T08:00:05Z"));
        assert!(describe(&active).ends_with(r#"  active  note "for bob""#));
    }

    #[tokio::test]
    async fn revoke_by_suffix() {
        let (_dir, pool) = pool().await;
        add_user(&pool, 1, "alice").await;
        let (invite, code) = create(&pool, NOW, DAY, None).await.unwrap();
        let (other, other_code) = create(&pool, NOW, DAY, None).await.unwrap();

        assert!(matches!(
            revoke(&pool, "zzzzzzzz", NOW).await,
            Err(RevokeError::Unknown)
        ));
        assert!(matches!(
            revoke(&pool, "", NOW).await,
            Err(RevokeError::Unknown)
        ));
        let revoked = revoke(&pool, &invite.short_id().to_lowercase(), NOW)
            .await
            .unwrap();
        assert_eq!(revoked.id, invite.id);
        assert!(!check_now(&pool, &code, NOW).await);
        assert!(matches!(
            revoke(&pool, &invite.id.to_string(), NOW).await,
            Err(RevokeError::AlreadyRevoked)
        ));

        assert!(consume_now(&pool, &other_code, 1, NOW).await);
        let err = revoke(&pool, &other.short_id(), NOW).await.unwrap_err();
        assert!(matches!(&err, RevokeError::Used { by: Some(by) } if by == "alice"));
        assert!(err.to_string().contains("already used by alice"), "{err}");
        // Still recorded as used by alice.
        let every = list(&pool, NOW, true).await.unwrap();
        assert!(
            every
                .iter()
                .any(|i| i.id == other.id && i.status == Status::Used)
        );

        let (old, _) = create(&pool, NOW, DAY, None).await.unwrap();
        assert!(matches!(
            revoke(&pool, &old.short_id(), NOW + 86_400).await,
            Err(RevokeError::Expired)
        ));
    }

    #[tokio::test]
    async fn ambiguous_suffix_is_refused() {
        let (_dir, pool) = pool().await;
        // IDs whose last 8 characters (40 bits) are the same.
        for (id, hash) in [(1u128, 1u8), (1 + (1 << 40), 2)] {
            sqlx::query(
                "INSERT INTO invites (id, code_hash, created_at, expires_at) VALUES (?, ?, 0, ?)",
            )
            .bind(Ulid(id).to_bytes().as_slice())
            .bind(vec![hash; 32])
            .bind(NOW + 1)
            .execute(&pool)
            .await
            .unwrap();
        }
        assert!(matches!(
            revoke(&pool, "00000001", NOW).await,
            Err(RevokeError::Ambiguous)
        ));
        assert_eq!(list(&pool, NOW, false).await.unwrap().len(), 2);
        revoke(&pool, "100000001", NOW).await.unwrap();
        assert_eq!(list(&pool, NOW, false).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn every_invalid_code_fails_alike() {
        let (_dir, pool) = pool().await;
        add_user(&pool, 1, "alice").await;
        add_user(&pool, 2, "bob").await;
        let (_, expired) = create(&pool, NOW - 86_400, Duration::from_secs(60), None)
            .await
            .unwrap();
        let (revoked, revoked_code) = create(&pool, NOW, DAY, None).await.unwrap();
        revoke(&pool, &revoked.short_id(), NOW).await.unwrap();
        let (_, used) = create(&pool, NOW, DAY, None).await.unwrap();
        assert!(consume_now(&pool, &used, 1, NOW).await);
        for code in [
            expired.as_str(),
            &revoked_code,
            &used,
            "7KQ2M-XW9PD-0RT4H-C6NBJ",
            "not a code",
            "",
        ] {
            assert!(!check_now(&pool, code, NOW).await, "{code:?}");
            assert!(!consume_now(&pool, code, 2, NOW).await, "{code:?}");
        }
        let users: Vec<Option<Vec<u8>>> =
            sqlx::query_scalar("SELECT used_by FROM invites WHERE used_by IS NOT NULL")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(users, [Some(vec![1; 16])]);
    }

    #[tokio::test]
    async fn consume_succeeds_once_and_accepts_retyped_codes() {
        let (_dir, pool) = pool().await;
        add_user(&pool, 1, "alice").await;
        let (_, code) = create(&pool, NOW, DAY, None).await.unwrap();
        let retyped = code.replace('-', "").to_lowercase();
        assert!(check_now(&pool, &retyped, NOW).await);
        assert!(consume_now(&pool, &retyped, 1, NOW).await);
        assert!(!consume_now(&pool, &code, 1, NOW).await);
        assert!(!check_now(&pool, &code, NOW).await);
        // Expiry is exclusive.
        let (_, code) = create(&pool, NOW, DAY, None).await.unwrap();
        assert!(!check_now(&pool, &code, NOW + 86_400).await);
        assert!(check_now(&pool, &code, NOW + 86_399).await);
    }

    #[tokio::test]
    async fn consume_rolls_back_with_its_transaction() {
        let (_dir, pool) = pool().await;
        add_user(&pool, 1, "alice").await;
        let (_, code) = create(&pool, NOW, DAY, None).await.unwrap();
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        assert!(consume(&mut tx, &code, &[1; 16], NOW).await.unwrap());
        drop(tx);
        assert!(check_now(&pool, &code, NOW).await);
    }
}
