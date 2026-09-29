//! The `codex app-server` JSON-over-stdio protocol: framing, an async RPC
//! client, and a process wrapper that spawns the real binary.

pub mod process;
pub mod protocol;
pub mod rpc;

pub use process::{AppServerProcess, spawn};
pub use rpc::RpcCallError;
