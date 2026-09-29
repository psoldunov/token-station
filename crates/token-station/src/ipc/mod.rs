//! The Unix-socket front door described in `docs/socket-api.md`.
//!
//! macOS has no session bus, so there the daemon serves the same interface over
//! a Unix-domain socket speaking newline-delimited JSON-RPC 2.0. Only the
//! framing differs: the methods, their arguments, their limits and their errors
//! are the D-Bus ones, because both adapters sit on the same [`crate::api::Api`].
//!
//! The module is built on every Unix, not only on macOS, so its tests run on
//! Linux too — the one place with a CI runner.

pub mod client;
pub mod conn;
pub mod hub;
pub mod protocol;
pub mod server;

pub use hub::Hub;
pub use protocol::{MAX_LINE_BYTES, PROTOCOL_VERSION};
pub use server::{BindError, Listener, bind, serve};
