//! Entry point: parse the command line and hand off to the library.

use std::process::ExitCode;

use clap::{CommandFactory, Parser};
use token_station::cli::{ALREADY_RUNNING_EXIT, Cli, Command, FAILURE_EXIT, SetupArgs};
use token_station::clock::system_clock;
use token_station::daemon::run::AlreadyRunning;
use token_station::daemon::{DaemonOptions, run as daemon};
use token_station::integration::{self, Session};
use token_station::paths::{Env, Paths};
use token_station::{init_logging, output, status, statusline, tray};

fn main() -> ExitCode {
    init_logging();
    let cli = Cli::parse();
    let paths = Paths::current();

    let result = match cli.command {
        // Double-clicking the AppImage should install it; a bare shell call should explain itself.
        None => default_command(),
        Some(Command::Statusline { wrap }) => {
            // The statusline hook never fails: Claude Code would show the error.
            statusline::run(wrap.as_deref(), &paths);
            Ok(())
        }
        Some(Command::Setup(args)) => setup(&args),
        Some(Command::Uninstall) => uninstall(),
        Some(other) => in_runtime(other, paths),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            output::error_line(&format!("{error:#}"));
            ExitCode::from(exit_code(&error))
        }
    }
}

/// `EX_TEMPFAIL` only for the one failure a restart cannot fix.
fn exit_code(error: &anyhow::Error) -> u8 {
    match error.downcast_ref::<AlreadyRunning>() {
        Some(_) => ALREADY_RUNNING_EXIT,
        None => FAILURE_EXIT,
    }
}

/// With no subcommand: set up when launched as an `AppImage`, else print the help.
fn default_command() -> anyhow::Result<()> {
    let session = Session::current();
    if session.appimage.is_some() {
        return setup(&SetupArgs::default());
    }
    Cli::command().print_help()?;
    output::line("");
    Ok(())
}

fn setup(args: &SetupArgs) -> anyhow::Result<()> {
    let summary = integration::setup(
        &args.options(),
        &Env::current(),
        &Session::current(),
        system_clock()(),
    )?;
    output::line(&summary);
    Ok(())
}

fn uninstall() -> anyhow::Result<()> {
    let summary = integration::uninstall(&Env::current(), &Session::current())?;
    output::line(&summary);
    Ok(())
}

/// Everything that needs the async runtime.
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
            Command::Tray => tray::run().await,
            Command::Statusline { .. } | Command::Setup(_) | Command::Uninstall => Ok(()),
        }
    })
}
