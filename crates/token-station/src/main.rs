//! Entry point: parse the command line and hand off to the library.

use std::process::ExitCode;

use clap::{CommandFactory, Parser};
use token_station::cli::{ALREADY_RUNNING_EXIT, Cli, Command, FAILURE_EXIT};
use token_station::daemon::run::AlreadyRunning;
use token_station::daemon::{DaemonOptions, run as daemon};
use token_station::paths::Paths;
use token_station::{init_logging, output, status, statusline};

#[cfg(not(target_os = "macos"))]
use token_station::cli::SetupArgs;
#[cfg(not(target_os = "macos"))]
use token_station::clock::system_clock;
#[cfg(not(target_os = "macos"))]
use token_station::integration::{self, Session};
#[cfg(not(target_os = "macos"))]
use token_station::paths::Env;

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
        #[cfg(not(target_os = "macos"))]
        Some(Command::Setup(args)) => setup(&args),
        #[cfg(not(target_os = "macos"))]
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
#[cfg(not(target_os = "macos"))]
fn default_command() -> anyhow::Result<()> {
    let session = Session::current();
    if session.appimage.is_some() {
        return setup(&SetupArgs::default());
    }
    print_help()
}

/// With no subcommand, explain the command line.
///
/// There is nothing to install from here on macOS: the app bundle carries this
/// binary as a helper, and the installer is whatever put the bundle in place.
#[cfg(target_os = "macos")]
fn default_command() -> anyhow::Result<()> {
    print_help()
}

fn print_help() -> anyhow::Result<()> {
    Cli::command().print_help()?;
    output::line("");
    Ok(())
}

#[cfg(not(target_os = "macos"))]
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

#[cfg(not(target_os = "macos"))]
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
            Command::Daemon {
                config,
                fixture,
                exit_on_stdin_close,
            } => {
                daemon::run(DaemonOptions {
                    config,
                    fixture,
                    exit_on_stdin_close,
                })
                .await
            }
            Command::Status { json } => status::status(&paths, json).await,
            Command::Refresh => status::refresh(&paths).await,
            #[cfg(not(target_os = "macos"))]
            Command::Tray => token_station::tray::run().await,
            #[cfg(not(target_os = "macos"))]
            Command::Statusline { .. } | Command::Setup(_) | Command::Uninstall => Ok(()),
            #[cfg(target_os = "macos")]
            Command::Statusline { .. } => Ok(()),
        }
    })
}
