//! Command-line surface.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Exit code for subcommands another release will fill in.
pub const NOT_IMPLEMENTED_EXIT: i32 = 2;
/// Exit code for "the bus name is already taken".
pub const ALREADY_RUNNING_EXIT: i32 = 1;

/// Plan usage for Claude Code and Codex, in your tray.
#[derive(Debug, Parser)]
#[command(name = "token-station", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the background service that owns `dev.soldunov.TokenStation`.
    Daemon {
        /// Config file to use instead of the XDG one.
        #[arg(long, value_name = "PATH")]
        config: Option<PathBuf>,
        /// Serve a recorded snapshot instead of the real CLIs.
        #[arg(long, value_name = "PATH")]
        fixture: Option<PathBuf>,
    },
    /// Print the current usage.
    Status {
        /// Print the raw snapshot JSON.
        #[arg(long)]
        json: bool,
    },
    /// Ask the running daemon to refresh now.
    Refresh,
    /// Claude Code statusline hook: reads the hook JSON on stdin.
    Statusline {
        /// Run this shell command with the same stdin and print its output.
        #[arg(long, value_name = "CMD")]
        wrap: Option<String>,
    },
    /// Standalone StatusNotifierItem tray icon.
    Tray,
    /// Install the user services and desktop integration.
    Setup,
    /// Remove what `setup` installed.
    Uninstall,
}

impl Command {
    /// Subcommands that are declared but not built yet.
    pub fn pending(&self) -> Option<&'static str> {
        match self {
            Command::Tray => Some("tray"),
            Command::Setup => Some("setup"),
            Command::Uninstall => Some("uninstall"),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Cli {
        Cli::parse_from(std::iter::once("token-station").chain(args.iter().copied()))
    }

    #[test]
    fn the_command_tree_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn daemon_takes_a_config_and_a_fixture() {
        let cli = parse(&[
            "daemon",
            "--config",
            "/tmp/c.toml",
            "--fixture",
            "/tmp/f.json",
        ]);
        match cli.command {
            Command::Daemon { config, fixture } => {
                assert_eq!(config, Some(PathBuf::from("/tmp/c.toml")));
                assert_eq!(fixture, Some(PathBuf::from("/tmp/f.json")));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn status_json_is_a_flag() {
        assert!(matches!(
            parse(&["status", "--json"]).command,
            Command::Status { json: true }
        ));
        assert!(matches!(
            parse(&["status"]).command,
            Command::Status { json: false }
        ));
    }

    #[test]
    fn statusline_wrap_takes_a_command() {
        match parse(&["statusline", "--wrap", "cat"]).command {
            Command::Statusline { wrap } => assert_eq!(wrap.as_deref(), Some("cat")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unfinished_subcommands_are_flagged() {
        assert_eq!(parse(&["tray"]).command.pending(), Some("tray"));
        assert_eq!(parse(&["setup"]).command.pending(), Some("setup"));
        assert_eq!(parse(&["uninstall"]).command.pending(), Some("uninstall"));
        assert_eq!(parse(&["refresh"]).command.pending(), None);
    }

    #[test]
    fn an_unknown_subcommand_is_rejected() {
        assert!(Cli::try_parse_from(["token-station", "fly"]).is_err());
    }
}
