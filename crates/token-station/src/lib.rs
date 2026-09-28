//! Token Station: the daemon that reads Claude Code and Codex plan usage and
//! publishes one JSON snapshot on the session bus, plus the CLI around it.
//!
//! Every front end (Plasma applet, GNOME extension, SNI tray) reads that snapshot;
//! this process is the only one that touches credentials, the network or local logs.

pub mod atomic;
pub mod backend;
pub mod cli;
pub mod clock;
pub mod coalesce;
pub mod daemon;
pub mod dbus;
pub mod fixture;
pub mod history;
pub mod integration;
pub mod notify;
pub mod output;
pub mod paths;
pub mod pricing_refresh;
pub mod publish;
pub mod settings;
pub mod status;
pub mod statusline;
pub mod tray;

/// Environment variable that selects the log filter (`tracing_subscriber` syntax).
pub const LOG_ENV: &str = "TOKEN_STATION_LOG";

/// Send `tracing` output to stderr, filtered by `$TOKEN_STATION_LOG` (default `info`).
pub fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env(LOG_ENV).unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logging_can_be_initialised_twice() {
        init_logging();
        init_logging();
    }
}
