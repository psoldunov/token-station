//! The long-running daemon.

#[cfg(not(target_os = "macos"))]
pub mod bus;
pub mod providers;
pub mod run;
pub mod scheduler;
#[cfg(unix)]
pub mod socket;
pub mod state;

pub use run::{DaemonOptions, run};
pub use state::Daemon;
