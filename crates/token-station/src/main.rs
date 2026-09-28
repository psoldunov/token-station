//! Entry point: parse the command line and hand off to the library.

use std::process::ExitCode;

use clap::{CommandFactory, Parser};
use token_station::cli::{ALREADY_RUNNING_EXIT, Cli, Command, SetupArgs};
use token_station::clock::system_clock;
use token_station::daemon::{DaemonOptions, run as daemon};
use token_station::integration::{self, Session};
use token_station::paths::{Env, Paths};
use token_station::{init_logging, status, statusline, tray};

fn main() -> ExitCode {
    init_logging();
    let cli = Cli::parse();
    let paths = Paths::current();

    let result = match cli.command {
        // Double-clicking the AppImage should install it; a bare shell call should explain itself.
        None => default_command(),
        Some(Command::Statusline { wrap }) => {
            statusline::run(wrap.as_deref(), &paths).map_err(anyhow::Error::from)
        }
        Some(Command::Setup(args)) => setup(&args),
        Some(Command::Uninstall) => uninstall(),
        Some(other) => in_runtime(other, paths),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::from(ALREADY_RUNNING_EXIT as u8)
        }
    }
}

/// With no subcommand: set up when launched as an AppImage, else print the help.
fn default_command() -> anyhow::Result<()> {
    let session = Session::current();
    if session.appimage.is_some() {
        return setup(&SetupArgs::default());
    }
    Cli::command().print_help()?;
    println!();
    Ok(())
}

fn setup(args: &SetupArgs) -> anyhow::Result<()> {
    let summary = integration::setup(
        &args.options(),
        &Env::current(),
        &Session::current(),
        system_clock()(),
    )?;
    println!("{summary}");
    Ok(())
}

fn uninstall() -> anyhow::Result<()> {
    let summary = integration::uninstall(&Env::current(), &Session::current())?;
    println!("{summary}");
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
