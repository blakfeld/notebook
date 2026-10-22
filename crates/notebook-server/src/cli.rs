//! Command-line interface. New operator commands are added to [`Command`].

use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};

/// Notebook sync server.
#[derive(Debug, Parser)]
#[command(name = "notebook-server", version, arg_required_else_help = true)]
pub struct Cli {
    /// Path to a TOML configuration file.
    #[arg(long, env = "NOTEBOOK_CONFIG", global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Apply pending migrations, then serve until SIGINT or SIGTERM.
    Serve,
    /// Apply pending migrations and exit.
    Migrate,
    /// Manage accounts.
    #[command(subcommand)]
    User(UserCommand),
    /// Manage the invite codes that registration requires.
    #[command(subcommand)]
    Invite(InviteCommand),
}

#[derive(Debug, Subcommand)]
pub enum InviteCommand {
    /// Create a single-use invite code and print it once.
    ///
    /// Only the code goes to stdout, so `$(notebook-server invite create)`
    /// captures it; the expiry goes to stderr.
    Create {
        /// How long the code stays valid: a number of minutes, hours, or
        /// days, such as 30m, 12h, or 7d (at most 365d).
        #[arg(long, value_name = "DURATION", default_value = "7d", value_parser = parse_duration)]
        expires: Duration,
        /// Recorded with the invite, such as who it is for.
        #[arg(long, value_name = "TEXT")]
        note: Option<String>,
    },
    /// List active invites (never their codes).
    List {
        /// Also list used, expired, and revoked invites.
        #[arg(long)]
        all: bool,
    },
    /// Revoke an active invite so it can't be redeemed.
    Revoke {
        /// The ID shown by `invite list`, or any unique ending of it.
        id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum UserCommand {
    /// Create an account with its vault, and print its recovery key once.
    ///
    /// Prompts twice for the passphrase without echoing it. The user's first
    /// sign-in from a client is an ordinary login.
    Add {
        /// 3 to 32 letters, digits, '.', '_', or '-'; case-insensitive.
        username: String,
        /// Mark the account as an administrator.
        #[arg(long)]
        admin: bool,
        /// Read the passphrase from one line of stdin instead of prompting
        /// (for scripted setups).
        #[arg(long)]
        passphrase_stdin: bool,
    },
    /// Show a user's vault storage usage and quota, or set the quota.
    ///
    /// A quota below current usage is allowed; it only blocks growth.
    Quota {
        /// The account's username; case-insensitive.
        username: String,
        /// The new quota: bytes, or a number with KiB, MiB, or GiB, such as
        /// 1GiB.
        #[arg(value_parser = parse_size)]
        size: Option<u64>,
    },
}

/// The longest invite lifetime.
const MAX_INVITE_DAYS: u64 = 365;

/// Parses `<n>m`, `<n>h`, or `<n>d`, from 1 minute to 365 days.
pub fn parse_duration(text: &str) -> Result<Duration, String> {
    let usage = "expected a number of minutes, hours, or days, such as 30m, 12h, or 7d";
    let unit = text.chars().last().ok_or(usage)?;
    let seconds = match unit {
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
        _ => return Err(usage.to_owned()),
    };
    let count = &text[..text.len() - 1];
    if count.is_empty() || !count.bytes().all(|b| b.is_ascii_digit()) {
        return Err(usage.to_owned());
    }
    let total = count
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(seconds));
    match total {
        Some(0) => Err("must be at least 1 minute".to_owned()),
        Some(total) if total <= MAX_INVITE_DAYS * 86_400 => Ok(Duration::from_secs(total)),
        _ => Err(format!("must be at most {MAX_INVITE_DAYS} days")),
    }
}

/// Parses a byte count, or a number with `KiB`, `MiB`, or `GiB`.
pub fn parse_size(text: &str) -> Result<u64, String> {
    let usage = "expected bytes, or a number with KiB, MiB, or GiB, such as 1GiB";
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    let (count, unit) = text.split_at(digits);
    let multiplier: u64 = match unit.trim_start() {
        "" => 1,
        "KiB" => 1 << 10,
        "MiB" => 1 << 20,
        "GiB" => 1 << 30,
        _ => return Err(usage.to_owned()),
    };
    if count.is_empty() {
        return Err(usage.to_owned());
    }
    let bytes = count
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(multiplier));
    match bytes {
        Some(0) => Err("must be at least 1 byte".to_owned()),
        Some(bytes) if i64::try_from(bytes).is_ok() => Ok(bytes),
        _ => Err("too large".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_subcommands_and_config() {
        let cli =
            Cli::try_parse_from(["notebook-server", "migrate", "--config", "a.toml"]).unwrap();
        assert!(matches!(cli.command, Command::Migrate));
        assert_eq!(cli.config, Some(PathBuf::from("a.toml")));
    }

    #[test]
    fn parses_user_add() {
        let cli = Cli::try_parse_from(["notebook-server", "user", "add", "alice"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::User(UserCommand::Add { ref username, admin: false, passphrase_stdin: false })
                if username == "alice"
        ));
        let cli = Cli::try_parse_from([
            "notebook-server",
            "user",
            "add",
            "root",
            "--admin",
            "--passphrase-stdin",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::User(UserCommand::Add {
                admin: true,
                passphrase_stdin: true,
                ..
            })
        ));
        assert!(Cli::try_parse_from(["notebook-server", "user", "add"]).is_err());
        assert!(Cli::try_parse_from(["notebook-server", "user"]).is_err());
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("30m"), Ok(Duration::from_secs(1800)));
        assert_eq!(parse_duration("12h"), Ok(Duration::from_secs(43_200)));
        assert_eq!(parse_duration("7d"), Ok(Duration::from_secs(604_800)));
        assert_eq!(parse_duration("1m"), Ok(Duration::from_secs(60)));
        assert_eq!(parse_duration("365d"), Ok(Duration::from_secs(31_536_000)));
        assert_eq!(parse_duration("8760h"), Ok(Duration::from_secs(31_536_000)));
        for zero in ["0m", "0h", "0d", "000d"] {
            assert!(
                parse_duration(zero).unwrap_err().contains("1 minute"),
                "{zero}"
            );
        }
        for oversize in ["366d", "8761h", "99999999999999999999d"] {
            assert!(
                parse_duration(oversize).unwrap_err().contains("365 days"),
                "{oversize}"
            );
        }
        for garbage in [
            "soon", "", "d", "7", "7s", "7 d", "-1d", "+1d", "1.5h", "7D", "é",
        ] {
            assert!(parse_duration(garbage).is_err(), "{garbage:?}");
        }
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_size("1048576"), Ok(1_048_576));
        assert_eq!(parse_size("1KiB"), Ok(1024));
        assert_eq!(parse_size("50MiB"), Ok(52_428_800));
        assert_eq!(parse_size("1GiB"), Ok(1_073_741_824));
        assert_eq!(parse_size("2 GiB"), Ok(2_147_483_648));
        for zero in ["0", "0GiB"] {
            assert!(parse_size(zero).unwrap_err().contains("1 byte"), "{zero}");
        }
        for oversize in [
            "9223372036854775808",
            "8589934592GiB",
            "99999999999999999999",
        ] {
            assert!(
                parse_size(oversize).unwrap_err().contains("too large"),
                "{oversize}"
            );
        }
        for garbage in ["", "GiB", "1GB", "1gib", "1.5GiB", "-1", "lots", " 1"] {
            assert!(parse_size(garbage).is_err(), "{garbage:?}");
        }
    }

    #[test]
    fn parses_invite_and_quota() {
        let cli = Cli::try_parse_from(["notebook-server", "invite", "create"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Invite(InviteCommand::Create { expires, note: None })
                if expires == Duration::from_secs(7 * 86_400)
        ));
        let cli = Cli::try_parse_from([
            "notebook-server",
            "invite",
            "create",
            "--expires",
            "12h",
            "--note",
            "for alice",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Invite(InviteCommand::Create { expires, note: Some(ref note) })
                if expires == Duration::from_secs(43_200) && note == "for alice"
        ));
        let err = Cli::try_parse_from(["notebook-server", "invite", "create", "--expires", "soon"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("--expires"), "{err}");
        assert!(matches!(
            Cli::try_parse_from(["notebook-server", "invite", "list", "--all"])
                .unwrap()
                .command,
            Command::Invite(InviteCommand::List { all: true })
        ));
        assert!(Cli::try_parse_from(["notebook-server", "invite", "revoke"]).is_err());

        let cli =
            Cli::try_parse_from(["notebook-server", "user", "quota", "alice", "1GiB"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::User(UserCommand::Quota { ref username, size: Some(1_073_741_824) })
                if username == "alice"
        ));
        let cli = Cli::try_parse_from(["notebook-server", "user", "quota", "alice"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::User(UserCommand::Quota { size: None, .. })
        ));
        assert!(Cli::try_parse_from(["notebook-server", "user", "quota", "alice", "big"]).is_err());
    }

    #[test]
    fn no_subcommand_is_an_error() {
        let err = Cli::try_parse_from(["notebook-server"]).unwrap_err();
        assert_eq!(err.exit_code(), 2);
        let err = Cli::try_parse_from(["notebook-server", "frobnicate"]).unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }
}
