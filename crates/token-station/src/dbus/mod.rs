//! The session-bus surface described in `docs/dbus-api.md`.

pub mod client;
pub mod service;

/// Well-known name the daemon owns.
pub const BUS_NAME: &str = "dev.soldunov.TokenStation";
/// The single exported object.
pub const OBJECT_PATH: &str = "/dev/soldunov/TokenStation";
/// The only interface on it.
pub const INTERFACE_NAME: &str = "dev.soldunov.TokenStation1";

/// Largest accepted `IngestClaudeStatusline` payload.
pub const MAX_STATUSLINE_BYTES: usize = 64 * 1024;
/// Longest accepted window id in `GetHistory`.
pub const MAX_WINDOW_ID_LEN: usize = 128;
