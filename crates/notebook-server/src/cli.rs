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
    fn no_subcommand_is_an_error() {
        let err = Cli::try_parse_from(["notebook-server"]).unwrap_err();
        assert_eq!(err.exit_code(), 2);
        let err = Cli::try_parse_from(["notebook-server", "frobnicate"]).unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }
}
