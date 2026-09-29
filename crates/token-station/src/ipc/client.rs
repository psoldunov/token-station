//! Talking to a running daemon over the socket (`status`, `refresh`,
//! `statusline`, and the tests).

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

use crate::ipc::protocol;

/// How long connecting may take before the daemon counts as absent.
pub const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);

/// Why a call did not produce a result.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("cannot talk to the daemon: {0}")]
    Io(#[from] std::io::Error),
    #[error("the daemon closed the connection")]
    Closed,
    #[error("the daemon sent an unreadable reply: {0}")]
    Unreadable(String),
    #[error("{message}")]
    Rpc {
        code: i32,
        message: String,
        data: Option<Value>,
    },
}

/// One connection to the daemon, with its own request ids.
pub struct Client {
    writer: OwnedWriteHalf,
    reader: BufReader<OwnedReadHalf>,
    next_id: u64,
}

impl Client {
    /// Connect to the daemon listening on `socket`.
    ///
    /// # Errors
    ///
    /// Returns an [`std::io::Error`] when nothing is listening, when the socket
    /// refuses the connection, or when `timeout` passes first.
    pub async fn connect(socket: &Path, timeout: Duration) -> std::io::Result<Client> {
        let stream = tokio::time::timeout(timeout, UnixStream::connect(socket))
            .await
            .map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("{} did not answer in time", socket.display()),
                )
            })??;
        let (read_half, writer) = stream.into_split();
        Ok(Client {
            writer,
            reader: BufReader::new(read_half),
            next_id: 1,
        })
    }

    /// Call `method` and wait for its result.
    ///
    /// Notifications that arrive while waiting are skipped: a caller that wants
    /// them uses [`Client::next_notification`] instead.
    ///
    /// # Errors
    ///
    /// Returns [`CallError::Rpc`] for an error object from the daemon,
    /// [`CallError::Closed`] when the connection ends first, and
    /// [`CallError::Io`] or [`CallError::Unreadable`] for a broken transport.
    pub async fn call(&mut self, method: &str, params: Option<Value>) -> Result<Value, CallError> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        // Built whole rather than indexed into: `params` is omitted entirely
        // when there is none, which is what a method that takes no arguments
        // expects to be sent.
        let request = match params {
            Some(params) => json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
            None => json!({"jsonrpc": "2.0", "id": id, "method": method}),
        };
        let line = serde_json::to_string(&request)
            .map_err(|error| CallError::Unreadable(error.to_string()))?;
        self.writer.write_all(line.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;

        loop {
            let message = self.read_message().await?;
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(CallError::Rpc {
                    code: error
                        .get("code")
                        .and_then(Value::as_i64)
                        .and_then(|code| i32::try_from(code).ok())
                        .unwrap_or(protocol::INTERNAL_ERROR),
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("the daemon reported an error")
                        .to_string(),
                    data: error.get("data").cloned(),
                });
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// The next notification the daemon pushes (after `Subscribe`).
    ///
    /// # Errors
    ///
    /// Returns [`CallError::Closed`] when the daemon closes the connection, and
    /// [`CallError::Io`] or [`CallError::Unreadable`] for a broken transport.
    pub async fn next_notification(&mut self) -> Result<(String, Value), CallError> {
        loop {
            let message = self.read_message().await?;
            if message.get("id").is_some() {
                continue;
            }
            let Some(method) = message.get("method").and_then(Value::as_str) else {
                continue;
            };
            return Ok((
                method.to_string(),
                message.get("params").cloned().unwrap_or(Value::Null),
            ));
        }
    }

    async fn read_message(&mut self) -> Result<Value, CallError> {
        let mut line = String::new();
        let read = self.reader.read_line(&mut line).await?;
        if read == 0 {
            return Err(CallError::Closed);
        }
        serde_json::from_str(&line).map_err(|error| CallError::Unreadable(error.to_string()))
    }
}

/// Connect, call once, and hang up.
///
/// # Errors
///
/// Returns whatever [`Client::connect`] and [`Client::call`] report.
pub async fn call_once(
    socket: &Path,
    method: &str,
    params: Option<Value>,
    timeout: Duration,
) -> Result<Value, CallError> {
    let mut client = Client::connect(socket, timeout).await?;
    client.call(method, params).await
}
