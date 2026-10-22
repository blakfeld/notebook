//! The OPAQUE server key file: the OPRF seed and server key pair, kept
//! outside the database.
//!
//! Without the OPRF seed, a copy of the database (including its Litestream
//! replica) can't be used to test passphrase guesses. The file is created on
//! first start, and its public-key fingerprint is recorded in the database so
//! a missing or wrong file is refused rather than silently replaced.

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

use notebook_core::auth::{SecureRng, Suite};
use opaque_ke::ServerSetup;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use super::store;

/// The OPAQUE server setup used by every handshake.
pub type Setup = ServerSetup<Suite>;

/// Prefixes the serialized setup, so a future format can be told apart.
const FILE_VERSION: u8 = 1;

const FINGERPRINT_KEY: &str = "opaque_key_fingerprint";

/// A new random setup.
pub fn generate() -> Setup {
    ServerSetup::new(&mut SecureRng)
}

/// SHA-256 of the server's public key.
pub fn fingerprint(setup: &Setup) -> [u8; 32] {
    Sha256::digest(setup.keypair().public().serialize()).into()
}

/// Loads the key file at `path`, or creates it when it is absent and no
/// accounts exist, then checks it against the fingerprint in the database.
pub async fn load_or_create(pool: &SqlitePool, path: &Path) -> Result<Setup, KeyFileError> {
    let err = |kind| KeyFileError {
        path: path.to_owned(),
        kind,
    };
    let (setup, created) = match std::fs::read(path) {
        Ok(bytes) => (parse(&bytes).ok_or_else(|| err(Kind::Invalid))?, false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if store::has_users(pool).await.map_err(|e| err(Kind::Db(e)))? {
                return Err(err(Kind::Missing));
            }
            let setup = generate();
            write_new(path, &setup).map_err(|e| err(Kind::Io(e)))?;
            tracing::info!("created OPAQUE key file {}", path.display());
            (setup, true)
        }
        Err(e) => return Err(err(Kind::Io(e))),
    };

    let actual = fingerprint(&setup);
    let recorded: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT value FROM server_meta WHERE key = ?")
            .bind(FINGERPRINT_KEY)
            .fetch_optional(pool)
            .await
            .map_err(|e| err(Kind::Db(e)))?;
    match recorded {
        Some(recorded) if recorded == actual => {}
        // A key generated just now (no accounts exist) replaces whatever an
        // earlier, account-less start recorded.
        Some(_) if !created => return Err(err(Kind::Mismatch)),
        _ => {
            sqlx::query("INSERT OR REPLACE INTO server_meta (key, value) VALUES (?, ?)")
                .bind(FINGERPRINT_KEY)
                .bind(actual.as_slice())
                .execute(pool)
                .await
                .map_err(|e| err(Kind::Db(e)))?;
        }
    }
    Ok(setup)
}

fn parse(bytes: &[u8]) -> Option<Setup> {
    match bytes.split_first() {
        Some((&FILE_VERSION, rest)) => Setup::deserialize(rest).ok(),
        _ => None,
    }
}

/// Writes the setup to a new file (never overwriting) readable only by its
/// owner.
fn write_new(path: &Path, setup: &Setup) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    let mut bytes = vec![FILE_VERSION];
    bytes.extend_from_slice(&setup.serialize());
    file.write_all(&bytes)?;
    file.sync_all()
}

/// Why the key file can't be used. The message names `opaque_key_path`.
#[derive(Debug)]
pub struct KeyFileError {
    pub path: PathBuf,
    pub kind: Kind,
}

#[derive(Debug)]
pub enum Kind {
    /// Absent while accounts exist.
    Missing,
    /// Its fingerprint differs from the one in the database.
    Mismatch,
    /// Not a key file this build can read.
    Invalid,
    Io(std::io::Error),
    Db(sqlx::Error),
}

impl fmt::Display for KeyFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let path = self.path.display();
        write!(f, "OPAQUE key file {path} (`opaque_key_path`): ")?;
        match &self.kind {
            Kind::Missing => f.write_str(
                "file is missing but accounts exist; restore it from backup. \
                 Refusing to generate a new key, which would lock out every user",
            ),
            Kind::Mismatch => f.write_str(
                "key does not match the database (it was recorded with a different key file); \
                 restore the right file from backup",
            ),
            Kind::Invalid => f.write_str("not a valid key file"),
            Kind::Io(e) => write!(f, "{e}"),
            Kind::Db(e) => write!(f, "database error: {e}"),
        }
    }
}

impl std::error::Error for KeyFileError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn pool(dir: &Path) -> SqlitePool {
        let pool = db::connect(&dir.join("db")).await.unwrap();
        db::migrate(&pool).await.unwrap();
        pool
    }

    async fn add_user(pool: &SqlitePool) {
        sqlx::query(
            "INSERT INTO users (user_id, username, opaque_record, recovery_verifier_hash, \
             created_at, updated_at) VALUES (x'01', 'alice', x'00', x'00', 0, 0)",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn first_start_creates_an_owner_only_file_and_records_it() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool(dir.path()).await;
        let path = dir.path().join("opaque.key");

        let setup = load_or_create(&pool, &path).await.unwrap();
        assert!(path.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let recorded: Vec<u8> = sqlx::query_scalar("SELECT value FROM server_meta WHERE key = ?")
            .bind(FINGERPRINT_KEY)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(recorded, fingerprint(&setup));
    }

    #[tokio::test]
    async fn restart_with_the_same_file_loads_it() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool(dir.path()).await;
        let path = dir.path().join("opaque.key");
        let first = load_or_create(&pool, &path).await.unwrap();
        add_user(&pool).await;
        let second = load_or_create(&pool, &path).await.unwrap();
        assert_eq!(first.serialize(), second.serialize());
    }

    #[tokio::test]
    async fn missing_file_with_accounts_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool(dir.path()).await;
        let path = dir.path().join("opaque.key");
        load_or_create(&pool, &path).await.unwrap();
        add_user(&pool).await;
        std::fs::remove_file(&path).unwrap();

        let err = load_or_create(&pool, &path).await.unwrap_err();
        assert!(matches!(err.kind, Kind::Missing), "{err}");
        assert!(err.to_string().contains("opaque_key_path"), "{err}");
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn key_file_from_another_deployment_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool(dir.path()).await;
        let path = dir.path().join("opaque.key");
        load_or_create(&pool, &path).await.unwrap();

        let other_dir = tempfile::tempdir().unwrap();
        let other_pool = self::pool(other_dir.path()).await;
        let other_path = other_dir.path().join("opaque.key");
        load_or_create(&other_pool, &other_path).await.unwrap();
        std::fs::copy(&other_path, &path).unwrap();

        let err = load_or_create(&pool, &path).await.unwrap_err();
        assert!(matches!(err.kind, Kind::Mismatch), "{err}");
        assert!(err.to_string().contains("does not match"), "{err}");
        assert!(err.to_string().contains("opaque_key_path"), "{err}");
    }

    #[tokio::test]
    async fn garbage_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool(dir.path()).await;
        let path = dir.path().join("opaque.key");
        std::fs::write(&path, b"not a key").unwrap();
        let err = load_or_create(&pool, &path).await.unwrap_err();
        assert!(matches!(err.kind, Kind::Invalid), "{err}");
        assert!(err.to_string().contains("opaque_key_path"), "{err}");
    }
}
