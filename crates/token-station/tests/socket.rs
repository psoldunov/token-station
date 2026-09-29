#![cfg(unix)]
//! The JSON-RPC socket from a client's side, end to end.
//!
//! macOS is where this runs in production and Linux is where the CI runner is,
//! so the module is built on every Unix and these tests exercise it on both.
//! What they assert is `docs/socket-api.md`: the methods, their errors, the
//! notification stream, and the one-daemon-per-user rule.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use token_station::api::Api;
use token_station::backend::{Backend, SetSettingsError};
use token_station::ipc::client::Client;
use token_station::ipc::hub::Hub;
use token_station::ipc::protocol::MAX_LINE_BYTES;
use token_station::ipc::server::{self, BindError};
use token_station::notify::Notifier;
use token_station::publish::{Publisher, SnapshotSink};
use ts_core::alerts::{Alert, AlertKind};
use ts_core::assemble::assemble;
use ts_core::config::Config;
use ts_core::{ProviderId, ProviderSnapshot, ProviderState, Snapshot};

const NOW: i64 = 1_790_596_800;
const CONNECT: Duration = Duration::from_secs(2);

/// A backend the test drives directly.
#[derive(Default)]
struct FakeBackend {
    refreshes: AtomicUsize,
    /// How long one refresh takes, so concurrent calls really do overlap.
    refresh_delay: Duration,
    settings: String,
    set_settings_result: Option<SetSettingsError>,
    ingest_result: Option<String>,
    ingested: std::sync::Mutex<Vec<String>>,
}

#[async_trait]
impl Backend for FakeBackend {
    async fn refresh(&self) {
        tokio::time::sleep(self.refresh_delay).await;
        self.refreshes.fetch_add(1, Ordering::SeqCst);
    }

    async fn history(&self, provider: ProviderId, window_id: &str, since: i64) -> Vec<(i64, f64)> {
        assert_eq!(provider, ProviderId::Claude);
        assert_eq!(window_id, "session");
        vec![(since, 34.0), (since + 300, 35.5)]
    }

    fn settings_json(&self) -> String {
        self.settings.clone()
    }

    async fn set_settings(&self, _json: &str) -> Result<(), SetSettingsError> {
        match &self.set_settings_result {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    async fn ingest_claude_statusline(&self, payload: &str) -> Result<(), String> {
        if let Some(error) = &self.ingest_result {
            return Err(error.clone());
        }
        self.ingested
            .lock()
            .expect("ingested")
            .push(payload.to_string());
        Ok(())
    }
}

fn snapshot(now: i64, state: ProviderState) -> Snapshot {
    assemble(
        1,
        now,
        &[ProviderSnapshot::empty(ProviderId::Claude, state)],
        &Config::default(),
    )
}

fn alert(percent: f64) -> Alert {
    Alert {
        provider: ProviderId::Claude,
        window_id: "session".into(),
        label: "Session".into(),
        kind: AlertKind::Warning,
        percent,
        resets_at: Some(NOW + 3_600),
    }
}

/// A daemon listening on a socket in a directory of its own.
struct Daemon {
    _dir: tempfile::TempDir,
    socket: PathBuf,
    publisher: Arc<Publisher>,
    hub: Arc<Hub>,
    backend: Arc<FakeBackend>,
    serving: tokio::task::JoinHandle<()>,
}

impl Daemon {
    fn start(backend: FakeBackend) -> Daemon {
        let dir = tempfile::tempdir().expect("temp dir");
        let socket = dir.path().join("daemon.sock");
        let listener =
            server::bind(&socket, &dir.path().join("daemon.lock")).expect("the socket binds");

        let backend = Arc::new(backend);
        let publisher = Publisher::new(snapshot(NOW, ProviderState::Loading));
        let hub = Hub::new();
        let (revision, json) = publisher.published();
        hub.seed(revision, json);
        publisher.attach(Arc::clone(&hub) as Arc<dyn SnapshotSink>);
        let api = Arc::new(Api::new(
            Arc::clone(&publisher),
            Arc::clone(&backend) as Arc<dyn Backend>,
        ));

        let serving = tokio::spawn(server::serve(listener, api, Arc::clone(&hub)));
        Daemon {
            _dir: dir,
            socket,
            publisher,
            hub,
            backend,
            serving,
        }
    }

    async fn client(&self) -> Client {
        Client::connect(&self.socket, CONNECT)
            .await
            .expect("a client connects")
    }

    /// A raw connection, for the framing the client would never produce.
    async fn raw(&self) -> BufReader<UnixStream> {
        BufReader::new(UnixStream::connect(&self.socket).await.expect("connected"))
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.serving.abort();
    }
}

/// Send one raw line and read one raw line back.
async fn exchange(connection: &mut BufReader<UnixStream>, line: &str) -> Value {
    connection
        .get_mut()
        .write_all(format!("{line}\n").as_bytes())
        .await
        .expect("written");
    let mut answer = String::new();
    connection.read_line(&mut answer).await.expect("read");
    serde_json::from_str(&answer).unwrap_or_else(|_| panic!("not JSON: {answer:?}"))
}

#[tokio::test]
async fn get_version_reports_the_protocol() {
    let daemon = Daemon::start(FakeBackend::default());
    let result = daemon
        .client()
        .await
        .call("GetVersion", None)
        .await
        .expect("a version");
    assert_eq!(result["protocol"], 1);
    assert_eq!(result["version"], env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn get_snapshot_carries_the_revision_and_the_state() {
    let daemon = Daemon::start(FakeBackend::default());
    let result = daemon
        .client()
        .await
        .call("GetSnapshot", None)
        .await
        .expect("a snapshot");
    assert_eq!(result["revision"], 1);
    assert_eq!(result["snapshot"]["revision"], 1);
    assert_eq!(result["snapshot"]["schemaVersion"], 1);
    assert_eq!(result["snapshot"]["providers"][0]["state"], "loading");
}

#[tokio::test]
async fn subscribe_answers_with_the_state_and_then_pushes_changes() {
    let daemon = Daemon::start(FakeBackend::default());
    let mut client = daemon.client().await;
    let first = client.call("Subscribe", None).await.expect("subscribed");
    assert_eq!(first["revision"], 1);

    // A change with the same content publishes nothing; a real one does.
    assert!(
        !daemon
            .publisher
            .publish(snapshot(NOW + 60, ProviderState::Loading))
            .await
    );
    assert!(
        daemon
            .publisher
            .publish(snapshot(NOW + 120, ProviderState::Ok))
            .await
    );

    let (method, params) = tokio::time::timeout(CONNECT, client.next_notification())
        .await
        .expect("a notification arrives")
        .expect("a notification");
    assert_eq!(method, "SnapshotChanged");
    assert_eq!(params["revision"], 2);
    assert_eq!(params["snapshot"]["revision"], 2);
    assert_eq!(params["snapshot"]["providers"][0]["state"], "ok");
}

/// The gap between answering `Subscribe` and starting to listen.
///
/// A revision published right after the answer went out must still be pushed.
/// Reading the hub separately from the subscription used to let the pump mark
/// it seen without ever sending it, and the client would sit on `loading` until
/// the next content change — minutes, on an idle machine.
#[tokio::test]
async fn a_revision_published_right_after_subscribe_is_still_delivered() {
    let daemon = Daemon::start(FakeBackend::default());
    let mut client = daemon.client().await;
    let first = client.call("Subscribe", None).await.expect("subscribed");
    assert_eq!(first["revision"], 1);

    // No pause: this races the pump starting up, which is the point.
    assert!(
        daemon
            .publisher
            .publish(snapshot(NOW + 60, ProviderState::Ok))
            .await
    );

    let (method, params) = tokio::time::timeout(CONNECT, client.next_notification())
        .await
        .expect("the new revision arrives")
        .expect("a notification");
    assert_eq!(method, "SnapshotChanged");
    assert_eq!(params["revision"], 2);
    assert_eq!(params["snapshot"]["providers"][0]["state"], "ok");
}

#[tokio::test]
async fn subscribing_twice_on_one_connection_answers_twice_and_pushes_once() {
    let daemon = Daemon::start(FakeBackend::default());
    let mut client = daemon.client().await;
    assert_eq!(
        client.call("Subscribe", None).await.expect("subscribed")["revision"],
        1
    );
    assert_eq!(
        client.call("Subscribe", None).await.expect("subscribed")["revision"],
        1
    );

    assert!(
        daemon
            .publisher
            .publish(snapshot(NOW + 60, ProviderState::Ok))
            .await
    );
    let (method, params) = tokio::time::timeout(CONNECT, client.next_notification())
        .await
        .expect("a notification")
        .expect("a notification");
    assert_eq!(method, "SnapshotChanged");
    assert_eq!(params["revision"], 2);

    // One stream, not two: a second `SnapshotChanged` for the same revision
    // would have the app redraw twice for one change.
    let second = tokio::time::timeout(Duration::from_millis(300), client.next_notification()).await;
    assert!(second.is_err(), "the same change was pushed twice");
}

#[tokio::test]
async fn a_notification_is_carried_out_and_never_answered() {
    let daemon = Daemon::start(FakeBackend::default());
    let mut connection = daemon.raw().await;
    connection
        .get_mut()
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"Refresh\"}\n")
        .await
        .expect("written");

    // The next answer is the one that asked for an answer; nothing came back
    // for the notification.
    let answer = exchange(
        &mut connection,
        r#"{"jsonrpc":"2.0","id":1,"method":"GetVersion"}"#,
    )
    .await;
    assert_eq!(answer["id"], 1);

    // It was still carried out — concurrently with the request that followed
    // it, which is why this waits rather than reads the counter on the spot.
    for _ in 0..40 {
        if daemon.backend.refreshes.load(Ordering::SeqCst) == 1 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the notification was never carried out");
}

#[tokio::test]
async fn an_unusable_request_is_answered_with_the_id_it_carried() {
    let daemon = Daemon::start(FakeBackend::default());
    let mut connection = daemon.raw().await;

    // JSON, with an id, and not a request: answering `null` would leave the
    // client waiting for id 11 for as long as it stays connected.
    let answer = exchange(&mut connection, r#"{"id":11,"method":"GetVersion"}"#).await;
    assert_eq!(answer["error"]["code"], -32600);
    assert_eq!(answer["id"], 11);

    let answer = exchange(
        &mut connection,
        r#"{"jsonrpc":"3.0","id":"abc","method":"GetVersion"}"#,
    )
    .await;
    assert_eq!(answer["id"], "abc");

    // Not JSON at all: there is no id to answer with, and the spec says null.
    let answer = exchange(&mut connection, "{not json").await;
    assert_eq!(answer["id"], Value::Null);
}

#[tokio::test]
async fn an_answer_still_reaches_a_client_that_half_closed_its_write_end() {
    let daemon = Daemon::start(FakeBackend {
        refresh_delay: Duration::from_millis(200),
        ..FakeBackend::default()
    });
    let stream = UnixStream::connect(&daemon.socket)
        .await
        .expect("connected");
    let mut connection = BufReader::new(stream);
    connection
        .get_mut()
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"Refresh\"}\n")
        .await
        .expect("written");
    // "I have nothing more to ask" is not "I have stopped listening": the app
    // may well say the first and wait for the answer it is owed.
    connection
        .get_mut()
        .shutdown()
        .await
        .expect("write half closed");

    let mut answer = String::new();
    tokio::time::timeout(CONNECT, connection.read_line(&mut answer))
        .await
        .expect("the answer arrives")
        .expect("read");
    let parsed: Value = serde_json::from_str(&answer).expect("json");
    assert_eq!(parsed["id"], 1);
    assert_eq!(parsed["result"], Value::Null);
}

#[tokio::test]
async fn an_alert_raised_before_anybody_subscribed_still_arrives() {
    let daemon = Daemon::start(FakeBackend::default());
    // Nobody is listening yet: the alert is kept rather than dropped.
    daemon.hub.notify(&alert(82.4), NOW).await;

    let mut client = daemon.client().await;
    client.call("Subscribe", None).await.expect("subscribed");
    let (method, params) = tokio::time::timeout(CONNECT, client.next_notification())
        .await
        .expect("the kept alert arrives")
        .expect("a notification");
    assert_eq!(method, "Alert");
    assert_eq!(params["provider"], "claude");
    assert_eq!(params["windowId"], "session");
    assert_eq!(params["kind"], "warning");
    assert_eq!(params["percent"], 82.4);
    assert_eq!(params["summary"], "Claude Code: Session at 82 %");
    assert_eq!(params["body"], "Resets in 1 h 0 min");
}

#[tokio::test]
async fn concurrent_refreshes_share_one_run() {
    let daemon = Daemon::start(FakeBackend {
        refresh_delay: Duration::from_millis(200),
        ..FakeBackend::default()
    });
    let mut first = daemon.client().await;
    let mut second = daemon.client().await;
    let (a, b) = tokio::join!(first.call("Refresh", None), second.call("Refresh", None));
    assert_eq!(a.expect("refreshed"), Value::Null);
    assert_eq!(b.expect("refreshed"), Value::Null);
    assert_eq!(
        daemon.backend.refreshes.load(Ordering::SeqCst),
        1,
        "the second caller waited for the first refresh instead of starting another"
    );
}

#[tokio::test]
async fn get_history_validates_its_arguments() {
    let daemon = Daemon::start(FakeBackend::default());
    let mut client = daemon.client().await;

    let points = client
        .call(
            "GetHistory",
            Some(json!({"provider": "claude", "windowId": "session", "since": NOW})),
        )
        .await
        .expect("a series");
    assert_eq!(points[0][0], NOW);
    assert_eq!(points[1][1], 35.5);

    // An unknown provider or window is an empty series, not an error: a front
    // end asking about something this build does not know draws nothing.
    for params in [
        json!({"provider": "gemini", "windowId": "session", "since": 0}),
        json!({"provider": "claude", "windowId": "", "since": 0}),
        json!({"provider": "claude", "windowId": "x".repeat(129), "since": 0}),
    ] {
        assert_eq!(
            client
                .call("GetHistory", Some(params.clone()))
                .await
                .expect("an empty series"),
            json!([]),
            "{params}"
        );
    }

    // Params of the wrong shape are a different answer entirely.
    let error = client
        .call("GetHistory", Some(json!({"provider": "claude"})))
        .await
        .expect_err("invalid params");
    assert!(error.to_string().contains("GetHistory"), "{error}");
}

#[tokio::test]
async fn a_rejected_settings_document_lists_its_problems() {
    let problems = vec![
        "general.limits_interval_secs must be 120..=86400".to_string(),
        "alerts thresholds must be within 1..=100".to_string(),
    ];
    let daemon = Daemon::start(FakeBackend {
        set_settings_result: Some(SetSettingsError::Rejected(problems.clone())),
        ..FakeBackend::default()
    });
    let mut connection = daemon.raw().await;
    let answer = exchange(
        &mut connection,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "SetSettings",
                "params": {"settings": {"general": {"limits_interval_secs": 1}}}})
        .to_string(),
    )
    .await;
    assert_eq!(answer["error"]["code"], -32602);
    assert_eq!(answer["error"]["data"]["problems"][0], problems[0]);
    assert_eq!(answer["error"]["data"]["problems"][1], problems[1]);
}

#[tokio::test]
async fn a_failed_settings_write_is_not_reported_as_bad_input() {
    let daemon = Daemon::start(FakeBackend {
        set_settings_result: Some(SetSettingsError::Failed(
            "cannot write config.toml: No space left".into(),
        )),
        ..FakeBackend::default()
    });
    let mut connection = daemon.raw().await;
    let answer = exchange(
        &mut connection,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "SetSettings", "params": {"settings": {}}})
            .to_string(),
    )
    .await;
    assert_eq!(answer["error"]["code"], -32000);
    assert!(
        answer["error"]["message"]
            .as_str()
            .expect("a message")
            .contains("No space left")
    );
}

#[tokio::test]
async fn get_settings_returns_the_config_as_an_object() {
    let daemon = Daemon::start(FakeBackend {
        settings: r#"{"general":{"limits_interval_secs":300}}"#.into(),
        ..FakeBackend::default()
    });
    let settings = daemon
        .client()
        .await
        .call("GetSettings", None)
        .await
        .expect("settings");
    assert_eq!(settings["general"]["limits_interval_secs"], 300);
}

#[tokio::test]
async fn an_oversized_statusline_is_refused_and_a_normal_one_is_taken() {
    let daemon = Daemon::start(FakeBackend::default());
    let mut client = daemon.client().await;

    client
        .call("IngestClaudeStatusline", Some(json!({"json": "{}"})))
        .await
        .expect("ingested");
    assert_eq!(daemon.backend.ingested.lock().expect("ingested").len(), 1);

    let too_big = "x".repeat(64 * 1024 + 1);
    let error = client
        .call("IngestClaudeStatusline", Some(json!({"json": too_big})))
        .await
        .expect_err("too large");
    assert!(error.to_string().contains("65536 bytes"), "{error}");
    assert_eq!(
        daemon.backend.ingested.lock().expect("ingested").len(),
        1,
        "the oversized payload never reached the backend"
    );
}

#[tokio::test]
async fn an_unknown_method_is_answered_and_the_connection_carries_on() {
    let daemon = Daemon::start(FakeBackend::default());
    let mut connection = daemon.raw().await;
    let answer = exchange(
        &mut connection,
        r#"{"jsonrpc":"2.0","id":1,"method":"Teleport"}"#,
    )
    .await;
    assert_eq!(answer["error"]["code"], -32601);

    let next = exchange(
        &mut connection,
        r#"{"jsonrpc":"2.0","id":2,"method":"GetVersion"}"#,
    )
    .await;
    assert_eq!(next["id"], 2);
}

#[tokio::test]
async fn a_line_that_is_not_json_keeps_the_connection_open() {
    let daemon = Daemon::start(FakeBackend::default());
    let mut connection = daemon.raw().await;

    let answer = exchange(&mut connection, "{not json").await;
    assert_eq!(answer["error"]["code"], -32700);
    assert_eq!(answer["id"], Value::Null);

    // JSON that is not a request is the other code, and also survivable.
    let answer = exchange(&mut connection, "[1,2,3]").await;
    assert_eq!(answer["error"]["code"], -32600);

    let next = exchange(
        &mut connection,
        r#"{"jsonrpc":"2.0","id":9,"method":"GetVersion"}"#,
    )
    .await;
    assert_eq!(next["id"], 9);
}

#[tokio::test]
async fn an_oversized_line_is_refused_and_the_connection_closes() {
    let daemon = Daemon::start(FakeBackend::default());
    let mut connection = daemon.raw().await;
    let huge = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"IngestClaudeStatusline","params":{{"json":"{}"}}}}"#,
        "x".repeat(MAX_LINE_BYTES)
    );
    let answer = exchange(&mut connection, &huge).await;
    assert_eq!(answer["error"]["code"], -32600);
    assert_eq!(answer["id"], Value::Null);

    // The framing is gone with the line, so the daemon hangs up rather than
    // guessing where the next request starts.
    let mut rest = String::new();
    connection.read_line(&mut rest).await.expect("read");
    assert!(rest.is_empty(), "the connection stayed open: {rest:?}");
}

#[tokio::test]
async fn only_one_daemon_holds_the_directory_and_a_stale_socket_is_cleared() {
    let dir = tempfile::tempdir().expect("temp dir");
    let socket = dir.path().join("daemon.sock");
    let lock = dir.path().join("daemon.lock");

    // What a daemon killed with SIGKILL leaves behind.
    std::fs::write(&socket, b"stale").expect("a stale socket");
    let first = server::bind(&socket, &lock).expect("the stale socket is cleared");
    assert!(UnixStream::connect(&socket).await.is_ok());

    let Err(error) = server::bind(&socket, &lock) else {
        panic!("a second daemon was allowed to start");
    };
    assert!(matches!(error, BindError::AlreadyRunning), "{error}");
    assert_eq!(
        error.to_string(),
        "another token-station daemon is already running"
    );

    // The first one going away frees both the socket and the lock. The retry
    // is for this test binary, not for the daemon: a `fork` on another thread
    // duplicates the lock's descriptor for the microseconds until it `exec`s.
    drop(first);
    assert!(!socket.exists());
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match server::bind(&socket, &lock) {
            Ok(listener) => {
                drop(listener);
                break;
            }
            Err(BindError::AlreadyRunning) if std::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => panic!("the next daemon never started: {error}"),
        }
    }
}

#[tokio::test]
async fn a_request_still_running_when_the_client_leaves_does_not_panic() {
    let daemon = Daemon::start(FakeBackend {
        refresh_delay: Duration::from_millis(300),
        ..FakeBackend::default()
    });
    let mut connection = daemon.raw().await;
    connection
        .get_mut()
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"Refresh\"}\n")
        .await
        .expect("written");
    // Hang up while the refresh is still in flight.
    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(connection);

    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        daemon.backend.refreshes.load(Ordering::SeqCst),
        1,
        "the refresh ran to its end"
    );
    // And the daemon is still answering.
    assert!(daemon.client().await.call("GetVersion", None).await.is_ok());
}
