//! Entry point: parse the command line and hand off to the library.

use std::process::ExitCode;

use clap::Parser;
use token_station::cli::{ALREADY_RUNNING_EXIT, Cli, Command, NOT_IMPLEMENTED_EXIT};
use token_station::daemon::{DaemonOptions, run as daemon};
use token_station::paths::Paths;
use token_station::{init_logging, status, statusline};

fn main() -> ExitCode {
    init_logging();
    let cli = Cli::parse();

    if let Some(name) = cli.command.pending() {
        eprintln!("`token-station {name}` is not implemented yet");
        return ExitCode::from(NOT_IMPLEMENTED_EXIT as u8);
    }

    let paths = Paths::current();
    let result = match cli.command {
        Command::Statusline { wrap } => {
            statusline::run(wrap.as_deref(), &paths).map_err(anyhow::Error::from)
        }
        other => in_runtime(other, paths),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::from(ALREADY_RUNNING_EXIT as u8)
        }
    }
}

/// Everything except `statusline` needs the async runtime.
fn in_runtime(command: Command, paths: Paths) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        match command {
            Command::Daemon { config, fixture } => {
                daemon::run(DaemonOptions { config, fixture }).await
            }
            Command::Status { json } => status::status(&paths, json).await,
            Command::Refresh => status::refresh().await,
            Command::Statusline { .. } | Command::Tray | Command::Setup | Command::Uninstall => {
                Ok(())
            }
        }
    })
}
