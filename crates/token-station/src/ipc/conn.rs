//! One client connection: read lines, answer them, push notifications.
//!
//! Requests on one connection are answered concurrently — a `Refresh` that takes
//! two seconds must not hold up the `GetSettings` behind it — so responses may
//! arrive out of order and clients match them by `id`, as the spec says. One
//! writer task owns the write half, which is what keeps two concurrent answers
//! from interleaving mid-line.

use std::io;
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::api::Api;
use crate::backend::SetSettingsError;
use crate::ipc::hub::{Hub, Subscription};
use crate::ipc::protocol::{
    self, HistoryParams, INVALID_PARAMS, INVALID_REQUEST, IngestParams, MAX_LINE_BYTES,
    METHOD_NOT_FOUND, PROTOCOL_VERSION, REQUEST_FAILED, Request, SettingsParams,
};

/// How many lines may wait for the writer before a producer has to slow down.
const WRITE_QUEUE: usize = 64;

/// How many requests one connection may have in flight at once.
///
/// A client that stops reading cannot otherwise be told to stop asking: every
/// unanswered request holds its serialized snapshot, and the menu bar app keeps
/// one connection open for days. At the cap the read loop waits for an answer
/// to go out before taking the next request, which is back pressure the socket
/// then applies to the client for us.
const MAX_IN_FLIGHT: usize = 32;

/// A JSON-RPC error object, before it is put on the wire.
struct RpcError {
    code: i32,
    message: String,
    data: Option<Value>,
}

impl RpcError {
    fn new(code: i32, message: impl Into<String>) -> RpcError {
        RpcError {
            code,
            message: message.into(),
            data: None,
        }
    }
}

impl From<SetSettingsError> for RpcError {
    fn from(error: SetSettingsError) -> RpcError {
        // `Rejected` is a statement about the document the caller sent and
        // `Failed` one about the daemon, exactly as on D-Bus. `data.problems`
        // is what a settings dialog lists beside the fields.
        let problems = error.problems();
        let code = match error {
            SetSettingsError::Rejected(_) => INVALID_PARAMS,
            SetSettingsError::Failed(_) => REQUEST_FAILED,
        };
        RpcError {
            code,
            message: error.to_string(),
            data: Some(json!({"problems": problems})),
        }
    }
}

/// One connection's moving parts, so the read loop stays a read loop.
struct Session {
    api: Arc<Api>,
    hub: Arc<Hub>,
    /// Lines queued for the one task that owns the write half.
    lines: mpsc::Sender<String>,
    /// Requests being answered right now.
    in_flight: JoinSet<()>,
    /// The notification stream, once this connection has subscribed.
    pump: Option<tokio::task::JoinHandle<()>>,
}

/// Serve one connection until the client goes away.
pub async fn handle(stream: UnixStream, api: Arc<Api>, hub: Arc<Hub>) {
    let (read_half, write_half) = stream.into_split();
    let (lines, queue) = mpsc::channel::<String>(WRITE_QUEUE);
    let writer = tokio::spawn(write_lines(write_half, queue));

    let mut reader = BufReader::new(read_half);
    let mut session = Session {
        api,
        hub,
        lines,
        in_flight: JoinSet::new(),
        pump: None,
    };

    loop {
        match read_line(&mut reader, MAX_LINE_BYTES).await {
            Ok(Line::Eof) | Err(_) => break,
            Ok(Line::TooLong) => {
                session.refuse_oversized().await;
                break;
            }
            Ok(Line::Bytes(bytes)) => session.take(&bytes).await,
        }
    }

    session.finish().await;
    let _ = writer.await;
}

impl Session {
    /// Answer one line, or start what will answer it.
    async fn take(&mut self, bytes: &[u8]) {
        let Some(request) = answer_or_parse(bytes, &self.lines).await else {
            return;
        };
        if request.method == "Subscribe" {
            subscribe(&request, &self.hub, &self.lines, &mut self.pump).await;
            return;
        }
        self.make_room().await;
        let api = Arc::clone(&self.api);
        let lines = self.lines.clone();
        self.in_flight
            .spawn(async move { answer(&api, request, &lines).await });
    }

    /// Reap what has finished, and wait when too much has not.
    ///
    /// Reaping here rather than at EOF is what keeps a connection that lives
    /// for days from holding every handle it ever spawned. The cap is back
    /// pressure: a client that stops reading cannot otherwise be told to stop
    /// asking, and every unanswered request holds its serialized snapshot.
    async fn make_room(&mut self) {
        while self.in_flight.try_join_next().is_some() {}
        while self.in_flight.len() >= MAX_IN_FLIGHT {
            if self.in_flight.join_next().await.is_none() {
                return;
            }
        }
    }

    /// Refuse a line longer than the cap.
    ///
    /// The rest of that line is unreadable and so is everything after it,
    /// because the framing went with it; the caller hangs up.
    async fn refuse_oversized(&self) {
        let answer = protocol::failure(
            &Value::Null,
            INVALID_REQUEST,
            &format!("a line may be at most {MAX_LINE_BYTES} bytes"),
            None,
        );
        let _ = self.lines.send(answer).await;
    }

    /// The client is gone, but a request it sent may still be running: its
    /// answer is owed for as long as the write half is open.
    ///
    /// Takes `self`, because the last thing it does is let go of the queue —
    /// and the writer task ends when the last sender does, which is what the
    /// caller then waits for.
    async fn finish(mut self) {
        while self.in_flight.join_next().await.is_some() {}
        if let Some(pump) = self.pump.take() {
            pump.abort();
        }
        drop(self.lines);
    }
}

/// Parse one line, answering a bad one on the spot.
///
/// A line that is not JSON is a client bug, not a protocol break: it is
/// answered and the connection carries on. The id is the one the line carried
/// when it can be read at all — a client matches answers by id, and a null one
/// leaves it waiting for a request that will never be answered again.
async fn answer_or_parse(bytes: &[u8], lines: &mpsc::Sender<String>) -> Option<Request> {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => {
            let answer = protocol::failure(
                &Value::Null,
                protocol::PARSE_ERROR,
                &format!("parse error: {error}"),
                None,
            );
            let _ = lines.send(answer).await;
            return None;
        }
    };
    match protocol::parse_request(text) {
        Ok(request) => Some(request),
        Err(error) => {
            let id = protocol::id_of(text);
            let answer = protocol::failure(&id, error.code(), &error.to_string(), None);
            let _ = lines.send(answer).await;
            None
        }
    }
}

/// Run one request and send its answer, if it asked for one.
async fn answer(api: &Api, request: Request, lines: &mpsc::Sender<String>) {
    let id = request.id.clone();
    let result = dispatch(api, request).await;
    // A request without an id is a notification: it is carried out and never
    // answered.
    let Some(id) = id else { return };
    let line = match result {
        Ok(value) => protocol::success(&id, &value),
        Err(error) => protocol::failure(&id, error.code, &error.message, error.data),
    };
    let _ = lines.send(line).await;
}

/// `Subscribe`: answer with the current state, then start pushing.
///
/// The answered state is taken out of the subscription's own receiver, and
/// taking it is what marks it seen. Reading the hub separately leaves a gap: a
/// revision published between the two reads would be marked seen by the pump
/// without ever being sent, and the client would sit on `loading` until the
/// next content change — which, on an idle machine, is minutes away.
async fn subscribe(
    request: &Request,
    hub: &Arc<Hub>,
    lines: &mpsc::Sender<String>,
    pump: &mut Option<tokio::task::JoinHandle<()>>,
) {
    let mut subscription = hub.subscribe();
    let state = Arc::clone(&subscription.snapshots.borrow_and_update());
    if let Some(id) = &request.id {
        let result = protocol::snapshot_params(state.revision, &state.json);
        let _ = lines.send(protocol::success(id, &result)).await;
    }
    // Subscribing twice on one connection is not an error; it just does not
    // start a second stream of the same notifications.
    if pump.is_none() {
        *pump = Some(tokio::spawn(push(subscription, lines.clone())));
    }
}

/// Feed one subscriber: the alerts it missed, then everything that follows.
async fn push(mut subscription: Subscription, lines: mpsc::Sender<String>) {
    for missed in std::mem::take(&mut subscription.missed) {
        if lines.send(missed).await.is_err() {
            return;
        }
    }
    loop {
        let line = tokio::select! {
            changed = subscription.snapshots.changed() => {
                if changed.is_err() {
                    return;
                }
                let state = Arc::clone(&subscription.snapshots.borrow_and_update());
                protocol::snapshot_notification(state.revision, &state.json)
            }
            alert = subscription.alerts.recv() => match alert {
                Ok(line) => line,
                // Too slow to keep up: the alerts it missed are gone, and the
                // ones after them are not.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                    tracing::warn!(missed, "a subscriber fell behind on alerts");
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            },
        };
        if lines.send(line).await.is_err() {
            return;
        }
    }
}

/// Carry out one request.
async fn dispatch(api: &Api, request: Request) -> Result<Value, RpcError> {
    let method = request.method.as_str();
    match method {
        "GetVersion" => Ok(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "protocol": PROTOCOL_VERSION,
        })),
        "GetSnapshot" => {
            let (revision, json) = api.published();
            Ok(protocol::snapshot_params(revision, &json))
        }
        "Refresh" => {
            api.refresh().await;
            Ok(Value::Null)
        }
        "GetHistory" => {
            let params: HistoryParams = protocol::params_of(method, request.params)
                .map_err(|error| RpcError::new(INVALID_PARAMS, error))?;
            // A `since` before the epoch is not a different question from "since
            // the beginning".
            let since = u64::try_from(params.since).unwrap_or(0);
            let points = api
                .history_json(&params.provider, &params.window_id, since)
                .await;
            Ok(as_value(&points))
        }
        "GetSettings" => Ok(as_value(&api.settings_json())),
        "SetSettings" => {
            let params: SettingsParams = protocol::params_of(method, request.params)
                .map_err(|error| RpcError::new(INVALID_PARAMS, error))?;
            let document = serde_json::to_string(&params.settings)
                .map_err(|error| RpcError::new(INVALID_PARAMS, format!("{method}: {error}")))?;
            api.set_settings(&document).await?;
            Ok(Value::Null)
        }
        "IngestClaudeStatusline" => {
            let params: IngestParams = protocol::params_of(method, request.params)
                .map_err(|error| RpcError::new(INVALID_PARAMS, error))?;
            api.ingest_claude_statusline(&params.json)
                .await
                .map_err(|error| RpcError::new(INVALID_PARAMS, error))?;
            Ok(Value::Null)
        }
        // `Subscribe` never reaches here: it is answered on the read loop, where
        // the connection's notification stream lives.
        other => Err(RpcError::new(
            METHOD_NOT_FOUND,
            format!("unknown method: {other}"),
        )),
    }
}

/// Nest JSON the daemon produced itself.
fn as_value(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|error| {
        tracing::error!(%error, "the daemon produced unreadable JSON");
        Value::Null
    })
}

/// Write queued lines until the client stops reading.
async fn write_lines(
    mut write_half: tokio::net::unix::OwnedWriteHalf,
    mut queue: mpsc::Receiver<String>,
) {
    while let Some(line) = queue.recv().await {
        if write_half.write_all(line.as_bytes()).await.is_err()
            || write_half.write_all(b"\n").await.is_err()
        {
            // The client closed its end; nothing left to say to it.
            return;
        }
    }
    let _ = write_half.shutdown().await;
}

/// One line off the wire.
#[derive(Debug, PartialEq, Eq)]
enum Line {
    Bytes(Vec<u8>),
    /// The client closed its end.
    Eof,
    /// Longer than the cap; the framing is lost with it.
    TooLong,
}

/// Read one newline-terminated line, refusing one longer than `max` bytes.
///
/// The cap is enforced while reading rather than after, so a client that sends a
/// gigabyte without a newline cannot make the daemon hold a gigabyte.
async fn read_line<R: AsyncBufRead + Unpin>(reader: &mut R, max: usize) -> io::Result<Line> {
    let mut line = Vec::new();
    loop {
        let (chunk, consumed, complete) = {
            let available = reader.fill_buf().await?;
            if available.is_empty() {
                return Ok(if line.is_empty() {
                    Line::Eof
                } else {
                    Line::Bytes(line)
                });
            }
            match available.iter().position(|byte| *byte == b'\n') {
                Some(index) => (
                    available.iter().take(index).copied().collect(),
                    index + 1,
                    true,
                ),
                None => (available.to_vec(), available.len(), false),
            }
        };
        reader.consume(consumed);
        line.extend_from_slice(&chunk);
        if line.len() > max {
            return Ok(Line::TooLong);
        }
        if complete {
            return Ok(Line::Bytes(line));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn read_all(input: &[u8], max: usize) -> Vec<Line> {
        let mut reader = BufReader::with_capacity(8, input);
        let mut lines = Vec::new();
        loop {
            let line = read_line(&mut reader, max).await.expect("read");
            let done = matches!(line, Line::Eof | Line::TooLong);
            lines.push(line);
            if done {
                return lines;
            }
        }
    }

    #[tokio::test]
    async fn lines_are_split_on_newlines_across_reads() {
        // The buffer is smaller than the lines, so every line spans several fills.
        let lines = read_all(b"first line here\nsecond line here\n", MAX_LINE_BYTES).await;
        assert_eq!(
            lines,
            vec![
                Line::Bytes(b"first line here".to_vec()),
                Line::Bytes(b"second line here".to_vec()),
                Line::Eof,
            ]
        );
    }

    #[tokio::test]
    async fn a_last_line_without_a_newline_is_still_a_line() {
        let lines = read_all(b"trailing", MAX_LINE_BYTES).await;
        assert_eq!(lines, vec![Line::Bytes(b"trailing".to_vec()), Line::Eof]);
    }

    #[tokio::test]
    async fn an_empty_line_is_a_line_and_not_an_eof() {
        let lines = read_all(b"\n", MAX_LINE_BYTES).await;
        assert_eq!(lines, vec![Line::Bytes(Vec::new()), Line::Eof]);
    }

    #[tokio::test]
    async fn an_oversized_line_stops_the_read() {
        let mut input = vec![b'x'; 40];
        input.push(b'\n');
        assert_eq!(read_all(&input, 16).await, vec![Line::TooLong]);
    }

    #[test]
    fn a_rejected_document_and_a_failed_write_map_to_different_codes() {
        let rejected: RpcError =
            SetSettingsError::Rejected(vec!["one".into(), "two".into()]).into();
        assert_eq!(rejected.code, INVALID_PARAMS);
        assert_eq!(rejected.message, "one; two");
        assert_eq!(rejected.data.expect("problems")["problems"][1], "two");

        let failed: RpcError = SetSettingsError::Failed("No space left".into()).into();
        assert_eq!(failed.code, REQUEST_FAILED);
        assert_eq!(failed.message, "No space left");
    }
}
