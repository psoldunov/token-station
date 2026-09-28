//! Command-line surface.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use crate::integration::DesktopChoice;

/// Exit code for "the bus name is already taken".
pub const ALREADY_RUNNING_EXIT: i32 = 1;

/// Plan usage for Claude Code and Codex, in your tray.
#[derive(Debug, Parser)]
#[command(name = "token-station", version, about)]
pub struct Cli {
    /// With no subcommand: run `setup` inside an AppImage, print help otherwise.
    #[command(subcommand)]
    pub command: Option<Command>,
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
    Setup(SetupArgs),
    /// Remove what `setup` installed.
    Uninstall,
}

/// Flags of `token-station setup`.
#[derive(Debug, Clone, Default, PartialEq, Eq, clap::Args)]
pub struct SetupArgs {
    /// Which front end to install; `auto` reads `$XDG_CURRENT_DESKTOP`.
    #[arg(long, value_enum, default_value_t = DesktopArg::Auto)]
    pub desktop: DesktopArg,
    /// Also point Claude Code's `statusLine` at this binary.
    #[arg(long)]
    pub claude_statusline: bool,
    /// Front-end payloads to install from (default: `$APPDIR/usr/share/token-station/integrations`).
    #[arg(long, value_name = "DIR")]
    pub payload_dir: Option<PathBuf>,
    /// Print the plan without changing anything.
    #[arg(long)]
    pub dry_run: bool,
}

/// `--desktop`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum DesktopArg {
    #[default]
    Auto,
    Kde,
    Gnome,
    Other,
}

impl From<DesktopArg> for DesktopChoice {
    fn from(value: DesktopArg) -> DesktopChoice {
        match value {
            DesktopArg::Auto => DesktopChoice::Auto,
            DesktopArg::Kde => DesktopChoice::Kde,
            DesktopArg::Gnome => DesktopChoice::Gnome,
            DesktopArg::Other => DesktopChoice::Other,
        }
    }
}

impl SetupArgs {
    /// The options the installer works from.
    pub fn options(&self) -> crate::integration::SetupOptions {
        crate::integration::SetupOptions {
            desktop: self.desktop.into(),
            claude_statusline: self.claude_statusline,
            payload_dir: self.payload_dir.clone(),
            dry_run: self.dry_run,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Command {
        Cli::parse_from(std::iter::once("token-station").chain(args.iter().copied()))
            .command
            .expect("a subcommand was given")
    }

    #[test]
    fn the_command_tree_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn daemon_takes_a_config_and_a_fixture() {
        let command = parse(&[
            "daemon",
            "--config",
            "/tmp/c.toml",
            "--fixture",
            "/tmp/f.json",
        ]);
        match command {
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
            parse(&["status", "--json"]),
            Command::Status { json: true }
        ));
        assert!(matches!(
            parse(&["status"]),
            Command::Status { json: false }
        ));
    }

    #[test]
    fn statusline_wrap_takes_a_command() {
        match parse(&["statusline", "--wrap", "cat"]) {
            Command::Statusline { wrap } => assert_eq!(wrap.as_deref(), Some("cat")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_subcommand_is_optional() {
        assert!(Cli::parse_from(["token-station"]).command.is_none());
        assert!(matches!(parse(&["tray"]), Command::Tray));
        assert!(matches!(parse(&["uninstall"]), Command::Uninstall));
    }

    #[test]
    fn setup_defaults_to_an_automatic_install() {
        let Command::Setup(args) = parse(&["setup"]) else {
            panic!("expected setup");
        };
        assert_eq!(args, SetupArgs::default());
        let options = args.options();
        assert_eq!(options.desktop, DesktopChoice::Auto);
        assert!(!options.claude_statusline && !options.dry_run);
        assert_eq!(options.payload_dir, None);
    }

    #[test]
    fn setup_takes_every_documented_flag() {
        let Command::Setup(args) = parse(&[
            "setup",
            "--desktop",
            "gnome",
            "--claude-statusline",
            "--payload-dir",
            "/tmp/payload",
            "--dry-run",
        ]) else {
            panic!("expected setup");
        };
        let options = args.options();
        assert_eq!(options.desktop, DesktopChoice::Gnome);
        assert!(options.claude_statusline);
        assert!(options.dry_run);
        assert_eq!(options.payload_dir, Some(PathBuf::from("/tmp/payload")));
    }

    #[test]
    fn an_unknown_desktop_is_rejected() {
        assert!(Cli::try_parse_from(["token-station", "setup", "--desktop", "cinnamon"]).is_err());
    }

    #[test]
    fn an_unknown_subcommand_is_rejected() {
        assert!(Cli::try_parse_from(["token-station", "fly"]).is_err());
    }
}
