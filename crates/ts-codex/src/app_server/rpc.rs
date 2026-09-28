//! Async client for the app-server protocol, generic over the transport.
//!
//! Requests and their responses are matched by `id` while notifications and
//! server-initiated requests (which we always reject) are handled inline by a
//! background reader task. This lets [`crate::app_server::process`] plug in a
//! real child process while tests plug in an in-memory [`tokio::io::duplex`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use super::protocol::{
    Incoming, RpcError, encode_error_response, encode_notification, encode_request, parse_line,
};

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum RpcCallError {
    #[error("app-server error {0}: {1}")]
    Remote(i64, String),
    #[error("app-server call timed out")]
    Timeout,
    #[error("app-server connection closed")]
    Closed,
}

/// Pending calls plus a `closed` flag, both guarded by one lock so a call
/// racing the connection's close always resolves instead of hanging forever.
#[derive(Default)]
struct PendingState {
    map: HashMap<i64, oneshot::Sender<Result<Value, RpcError>>>,
    closed: bool,
}

type Pending = Arc<Mutex<PendingState>>;

/// A live connection to one app-server process (or test transport).
///
/// Dropping the client aborts its background reader/writer tasks; it does not
/// stop a child process, which is [`super::process::AppServerProcess`]'s job.
pub struct AppServerClient {
    outgoing: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: AtomicI64,
    reader_task: JoinHandle<()>,
    writer_task: JoinHandle<()>,
}

impl AppServerClient {
    pub fn connect<R, W>(reader: R, writer: W) -> AppServerClient
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (outgoing, mut outgoing_rx) = mpsc::unbounded_channel::<String>();
        let pending: Pending = Arc::new(Mutex::new(PendingState::default()));

        let writer_task = tokio::spawn(async move {
            let mut writer = writer;
            while let Some(line) = outgoing_rx.recv().await {
                if writer.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                if writer.write_all(b"\n").await.is_err() {
                    break;
                }
                if writer.flush().await.is_err() {
                    break;
                }
            }
        });

        let reply_tx = outgoing.clone();
        let reader_pending = pending.clone();
        let reader_task = tokio::spawn(async move {
            read_loop(reader, reply_tx, reader_pending).await;
        });

        AppServerClient {
            outgoing,
            pending,
            next_id: AtomicI64::new(1),
            reader_task,
            writer_task,
        }
    }

    /// Call a method and await its response, or `RpcCallError::Timeout` after `timeout`.
    pub async fn call<P: Serialize>(
        &self,
        method: &str,
        params: P,
        timeout: Duration,
    ) -> Result<Value, RpcCallError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        {
            let mut state = self.pending.lock().unwrap();
            if state.closed {
                return Err(RpcCallError::Closed);
            }
            state.map.insert(id, tx);
        }
        let line = encode_request(id, method, &params);
        if self.outgoing.send(line).is_err() {
            self.pending.lock().unwrap().map.remove(&id);
            return Err(RpcCallError::Closed);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(e))) => Err(RpcCallError::Remote(e.code, e.message)),
            Ok(Err(_)) => Err(RpcCallError::Closed),
            Err(_) => {
                self.pending.lock().unwrap().map.remove(&id);
                Err(RpcCallError::Timeout)
            }
        }
    }

    /// Send a one-way notification (e.g. `initialized`).
    pub fn notify<P: Serialize>(&self, method: &str, params: P) {
        let _ = self.outgoing.send(encode_notification(method, &params));
    }
}

impl Drop for AppServerClient {
    fn drop(&mut self) {
        self.reader_task.abort();
        self.writer_task.abort();
    }
}

async fn read_loop<R: AsyncRead + Unpin>(
    reader: R,
    reply_tx: mpsc::UnboundedSender<String>,
    pending: Pending,
) {
    let mut lines = BufReader::new(reader).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(e) => {
                tracing::debug!(error = %e, "app-server stdout read failed");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        dispatch_line(&line, &reply_tx, &pending);
    }
    // Connection closed: dropping each sender (rather than sending through
    // it) makes every waiting `call()` see a `RecvError`, which maps to
    // `RpcCallError::Closed` instead of a synthetic remote error.
    let mut state = pending.lock().unwrap();
    state.closed = true;
    state.map.clear();
}

fn dispatch_line(line: &str, reply_tx: &mpsc::UnboundedSender<String>, pending: &Pending) {
    match parse_line(line) {
        Ok(Incoming::Result { id, result }) => reply(pending, id, Ok(result)),
        Ok(Incoming::Error { id, error }) => reply(pending, id, Err(error)),
        Ok(Incoming::ServerRequest { id, method }) => {
            tracing::debug!(method = %method, "rejecting server-initiated request");
            let _ = reply_tx.send(encode_error_response(id, -32601, "method not found"));
        }
        Ok(Incoming::Notification { method, .. }) => {
            tracing::debug!(method = %method, "ignoring app-server notification");
        }
        Err(e) => tracing::debug!(error = %e, line, "unparseable app-server line"),
    }
}

fn reply(pending: &Pending, id: i64, outcome: Result<Value, RpcError>) {
    if let Some(sender) = pending.lock().unwrap().map.remove(&id) {
        let _ = sender.send(outcome);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn client_over_duplex(
        server_script: Vec<String>,
    ) -> (AppServerClient, tokio::io::DuplexStream) {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(client_io);
        let client = AppServerClient::connect(client_read, client_write);
        let _ = server_script;
        (client, server_io)
    }

    #[tokio::test]
    async fn matches_response_by_id_amid_notifications() {
        let (client, mut server) = client_over_duplex(vec![]);
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = [0u8; 4096];
            let _ = server.read(&mut buf).await; // the "ping" request
            server
                .write_all(b"{\"method\":\"account/updated\",\"params\":{}}\n")
                .await
                .unwrap();
            server
                .write_all(b"{\"id\":1,\"result\":{\"ok\":true}}\n")
                .await
                .unwrap();
        });
        let result = client
            .call("ping", json!({}), Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(result, json!({"ok": true}));
    }

    #[tokio::test]
    async fn rejects_server_initiated_request_with_method_not_found() {
        let (_client, mut server) = client_over_duplex(vec![]);
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        server
            .write_all(b"{\"id\":42,\"method\":\"weird/request\",\"params\":{}}\n")
            .await
            .unwrap();
        let mut buf = [0u8; 4096];
        let n = server.read(&mut buf).await.unwrap();
        let reply = String::from_utf8_lossy(&buf[..n]);
        let value: Value = serde_json::from_str(reply.trim()).unwrap();
        assert_eq!(value["id"], 42);
        assert_eq!(value["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn maps_remote_error_response() {
        let (client, mut server) = client_over_duplex(vec![]);
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = [0u8; 4096];
            let _ = server.read(&mut buf).await;
            server
                .write_all(
                    b"{\"id\":1,\"error\":{\"code\":-32600,\"message\":\"unknown variant\"}}\n",
                )
                .await
                .unwrap();
        });
        let err = client
            .call("bogus", json!({}), Duration::from_secs(2))
            .await
            .unwrap_err();
        assert_eq!(err, RpcCallError::Remote(-32600, "unknown variant".into()));
    }

    #[tokio::test]
    async fn call_times_out_when_no_response_arrives() {
        let (client, _server) = client_over_duplex(vec![]);
        let err = client
            .call("slow", json!({}), Duration::from_millis(20))
            .await
            .unwrap_err();
        assert_eq!(err, RpcCallError::Timeout);
    }

    #[tokio::test]
    async fn call_fails_when_connection_closes() {
        let (client, server) = client_over_duplex(vec![]);
        drop(server);
        let err = client
            .call("anything", json!({}), Duration::from_secs(2))
            .await
            .unwrap_err();
        assert!(matches!(err, RpcCallError::Closed));
    }

    #[tokio::test]
    async fn notify_sends_no_id() {
        let (client, mut server) = client_over_duplex(vec![]);
        client.notify("initialized", Value::Null);
        use tokio::io::AsyncReadExt;
        let mut buf = [0u8; 4096];
        let n = server.read(&mut buf).await.unwrap();
        let line = String::from_utf8_lossy(&buf[..n]);
        assert_eq!(line.trim(), r#"{"method":"initialized","params":null}"#);
    }
}
