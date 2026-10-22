//! Notebook sync server binary: parse the command line, load configuration,
//! initialize logging, and dispatch the subcommand.

use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use notebook_server::cli::{Cli, Command, UserCommand};
use notebook_server::config::{Config, LogFormat};
use notebook_server::{app, db, logging, user};

fn main() -> ExitCode {
    let cli = Cli::parse();

    let config = match Config::load(cli.config.as_deref()) {
        Ok(config) => config,
        Err(e) => {
            logging::init("info", LogFormat::Text);
            tracing::error!("{e}");
            return ExitCode::FAILURE;
        }
    };
    logging::init(&config.log_level, config.log_format);

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            tracing::error!("cannot start async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(async {
        match cli.command {
            Command::Serve => app::serve(config).await,
            Command::Migrate => migrate(config).await,
            Command::User(UserCommand::Add {
                username,
                admin,
                passphrase_stdin,
            }) => user::add(config, &username, admin, passphrase_stdin).await,
        }
    });
    // Cancels connection tasks left after the grace period instead of
    // waiting on them.
    runtime.shutdown_timeout(Duration::from_secs(1));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e}");
            ExitCode::FAILURE
        }
    }
}

async fn migrate(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let pool = db::connect(&config.database_path).await?;
    let result = db::migrate(&pool).await;
    pool.close().await;
    result?;
    tracing::info!("database {} is up to date", config.database_path.display());
    Ok(())
}
