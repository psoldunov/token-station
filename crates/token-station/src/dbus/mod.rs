//! The session-bus surface described in `docs/dbus-api.md`.

pub mod client;
pub mod service;
pub mod sink;

/// Well-known name the daemon owns.
pub const BUS_NAME: &str = "dev.soldunov.TokenStation";
/// The single exported object.
pub const OBJECT_PATH: &str = "/dev/soldunov/TokenStation";
/// The only interface on it.
pub const INTERFACE_NAME: &str = "dev.soldunov.TokenStation1";

pub use crate::api::{MAX_STATUSLINE_BYTES, MAX_WINDOW_ID_LEN};
