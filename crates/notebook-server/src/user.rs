//! `notebook-server user add`: creates a complete account (user, vault, key
//! bundle, recovery verifier) by running the client side of registration in
//! process, so the user's first sign-in is an ordinary login.

use std::fmt;
use std::io::{BufRead, IsTerminal};

use notebook_core::auth::api::RegisterStartResponse;
use notebook_core::auth::{AuthError, Device, Registration, Suite, Username};
use opaque_ke::{RegistrationRequest, RegistrationUpload, ServerRegistration};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use zeroize::Zeroizing;

use crate::auth::store::new_user_id;
use crate::auth::store::{self, CreateError};
use crate::auth::{Setup, keyfile, unix_now};
use crate::config::Config;
use crate::{db, quota};

#[derive(Debug)]
pub enum UserError {
    InvalidUsername,
    UsernameTaken,
    PassphraseTooShort,
    PassphraseMismatch,
    /// No terminal to prompt on, and `--passphrase-stdin` wasn't given.
    NotATerminal,
    Io(std::io::Error),
    Db(sqlx::Error),
    Other(String),
}

impl fmt::Display for UserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUsername => AuthError::InvalidUsername.fmt(f),
            Self::UsernameTaken => f.write_str("that username is already taken"),
            Self::PassphraseTooShort => AuthError::PassphraseTooShort.fmt(f),
            Self::PassphraseMismatch => f.write_str("the passphrases did not match"),
            Self::NotATerminal => f.write_str(
                "stdin is not a terminal; pass --passphrase-stdin to read the passphrase from it",
            ),
            Self::Io(e) => write!(f, "cannot read passphrase: {e}"),
            Self::Db(e) => write!(f, "database error: {e}"),
            Self::Other(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for UserError {}

impl From<sqlx::Error> for UserError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e)
    }
}

/// The `user add` command: load config and database, load or create the key
/// file (same rules as `serve`), read the passphrase, create the account, and
/// print the recovery key once.
pub async fn add(
    config: Config,
    username: &str,
    is_admin: bool,
    passphrase_stdin: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // Validate before prompting, so a typo doesn't cost a passphrase entry.
    let name = Username::parse(username).map_err(|_| UserError::InvalidUsername)?;
    let passphrase = if passphrase_stdin {
        read_line(&mut std::io::stdin().lock())?
    } else {
        if !std::io::stdin().is_terminal() {
            return Err(UserError::NotATerminal.into());
        }
        let prompt = |label: &str| rpassword::prompt_password(label).map(Zeroizing::new);
        confirm(
            prompt("Passphrase: ").map_err(UserError::Io)?,
            prompt("Repeat passphrase: ").map_err(UserError::Io)?,
        )?
    };

    let pool = db::connect(&config.database_path).await?;
    let result = async {
        db::migrate(&pool).await?;
        let setup = keyfile::load_or_create(&pool, &config.opaque_key_path).await?;
        Ok::<_, Box<dyn std::error::Error>>(
            create(
                &pool,
                &setup,
                name.as_str(),
                &passphrase,
                is_admin,
                config.default_quota(),
            )
            .await?,
        )
    }
    .await;
    pool.close().await;
    let recovery_key = result?;

    println!("Created {name}{}.", if is_admin { " (admin)" } else { "" });
    println!();
    println!("Recovery key (shown once; store it somewhere safe):");
    println!();
    println!("    {}", *recovery_key);
    println!();
    println!(
        "Anyone with this key can reset the passphrase and read the notes. \
         Without it, a forgotten passphrase means the notes are lost."
    );
    Ok(())
}

/// Creates the account, with a vault quota of `quota_bytes`, and returns its
/// recovery key for display. Nothing is created on error. Needs no invite:
/// the account goes straight to [`store::create_account`], never through the
/// HTTP handlers.
pub async fn create(
    pool: &SqlitePool,
    setup: &Setup,
    username: &str,
    passphrase: &str,
    is_admin: bool,
    quota_bytes: i64,
) -> Result<Zeroizing<String>, UserError> {
    let name = Username::parse(username).map_err(|_| UserError::InvalidUsername)?;
    let mut conn = pool.acquire().await?;
    if store::find_by_username(&mut conn, name.as_str())
        .await?
        .is_some()
    {
        return Err(UserError::UsernameTaken);
    }
    drop(conn);

    let (registration, start) =
        Registration::start(name.as_str(), passphrase, "").map_err(|e| match e {
            AuthError::PassphraseTooShort => UserError::PassphraseTooShort,
            AuthError::InvalidUsername => UserError::InvalidUsername,
            e => UserError::Other(e.to_string()),
        })?;
    let internal = |what: &str| UserError::Other(format!("registration failed: {what}"));
    let request = RegistrationRequest::<Suite>::deserialize(&start.registration_request)
        .map_err(|_| internal("request"))?;
    let response = ServerRegistration::start(setup, request, name.as_str().as_bytes())
        .map_err(|_| internal("server start"))?;
    let now = unix_now();
    let now_ms = u64::try_from(now).unwrap_or(0).saturating_mul(1000);
    let (finish, account) = registration
        .finish(
            &RegisterStartResponse {
                registration_response: response.message.serialize().to_vec(),
            },
            Device::default(),
            now_ms,
        )
        .map_err(|e| UserError::Other(e.to_string()))?;
    let upload = RegistrationUpload::<Suite>::deserialize(&finish.registration_upload)
        .map_err(|_| internal("upload"))?;
    let record = ServerRegistration::finish(upload).serialize();

    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let created = store::create_account(
        &mut tx,
        &store::NewAccount {
            user_id: new_user_id(now),
            username: name.as_str(),
            opaque_record: &record,
            recovery_verifier_hash: Sha256::digest(&finish.recovery_verifier).into(),
            is_admin,
            vault_id: account.vault_id.to_bytes(),
            key_bundle: &finish.key_bundle,
            now,
        },
        quota_bytes,
    )
    .await;
    match created {
        Ok(()) => {}
        Err(CreateError::UsernameTaken) => return Err(UserError::UsernameTaken),
        Err(CreateError::VaultTaken) => return Err(internal("vault id collision")),
        Err(CreateError::Db(e)) => return Err(e.into()),
    }
    tx.commit().await?;
    Ok(account.recovery_key.to_display())
}

/// The `user quota` command: prints the vault's usage and quota, after
/// setting the quota to `size` bytes if given. Unknown users are an error.
pub async fn quota(
    config: Config,
    username: &str,
    size: Option<u64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let name = Username::parse(username).map_err(|_| UserError::InvalidUsername)?;
    let pool = db::open(&config.database_path).await?;
    let result = async {
        let mut conn = pool.acquire().await?;
        match size {
            Some(size) => {
                let size = i64::try_from(size).unwrap_or(i64::MAX);
                quota::set(&mut conn, name.as_str(), size).await
            }
            None => quota::usage(&mut conn, name.as_str()).await,
        }
    }
    .await;
    pool.close().await;
    let Some(usage) = result? else {
        return Err(UserError::Other(format!("no user named {name}")).into());
    };
    println!(
        "{name}: {} of {} bytes used",
        usage.used_bytes, usage.quota_bytes
    );
    Ok(())
}

/// The passphrase, if both prompts agree.
pub fn confirm(
    first: Zeroizing<String>,
    second: Zeroizing<String>,
) -> Result<Zeroizing<String>, UserError> {
    if *first == *second {
        Ok(first)
    } else {
        Err(UserError::PassphraseMismatch)
    }
}

/// Reads one line, without its line ending, for `--passphrase-stdin`.
pub fn read_line(input: &mut impl BufRead) -> Result<Zeroizing<String>, UserError> {
    let mut line = Zeroizing::new(String::new());
    input.read_line(&mut line).map_err(UserError::Io)?;
    let len = line.trim_end_matches(['\n', '\r']).len();
    line.truncate(len);
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn z(s: &str) -> Zeroizing<String> {
        Zeroizing::new(s.to_owned())
    }

    #[test]
    fn prompts_must_match() {
        assert_eq!(*confirm(z("same"), z("same")).unwrap(), "same");
        assert!(matches!(
            confirm(z("one"), z("two")),
            Err(UserError::PassphraseMismatch)
        ));
    }

    #[test]
    fn stdin_line_drops_only_the_line_ending() {
        let mut input = &b"  spaced passphrase \r\nnext"[..];
        assert_eq!(*read_line(&mut input).unwrap(), "  spaced passphrase ");
        let mut input = &b"no newline"[..];
        assert_eq!(*read_line(&mut input).unwrap(), "no newline");
    }
}
