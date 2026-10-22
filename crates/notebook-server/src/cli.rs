//! Command-line interface. New operator commands are added to [`Command`].

use std::path::PathBuf;

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
    fn no_subcommand_is_an_error() {
        let err = Cli::try_parse_from(["notebook-server"]).unwrap_err();
        assert_eq!(err.exit_code(), 2);
        let err = Cli::try_parse_from(["notebook-server", "frobnicate"]).unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }
}
