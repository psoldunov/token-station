//! End-to-end checks of `dev.soldunov.TokenStation1` on a private session bus.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use token_station::backend::{Backend, SetSettingsError};
use token_station::dbus::client::TokenStationProxy;
use token_station::dbus::{BUS_NAME, OBJECT_PATH, service};
use token_station::publish::Publisher;
use ts_core::assemble::assemble;
use ts_core::config::Config;
use ts_core::{ProviderId, ProviderSnapshot, ProviderState, Snapshot};
use zbus::export::futures_core::Stream;
use zbus::fdo::{RequestNameFlags, RequestNameReply};

/// Backend that records what the interface asked it to do.
#[derive(Default)]
struct FakeBackend {
    refreshes: AtomicUsize,
    settings: std::sync::Mutex<Config>,
    last_statusline: std::sync::Mutex<Option<String>>,
}

#[async_trait]
impl Backend for FakeBackend {
    async fn refresh(&self) {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(80)).await;
    }

    async fn history(&self, provider: ProviderId, window_id: &str, since: i64) -> Vec<(i64, f64)> {
        if provider != ProviderId::Claude || window_id != "session" {
            return Vec::new();
        }
        vec![(since, 10.0), (since + 60, 20.5)]
    }

    fn settings_json(&self) -> String {
        self.settings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .to_json()
    }

    async fn set_settings(&self, json: &str) -> Result<(), SetSettingsError> {
        let next =
            token_station::settings::parse_settings(json).map_err(SetSettingsError::Rejected)?;
        *self
            .settings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
        Ok(())
    }

    async fn ingest_claude_statusline(&self, payload: &str) -> Result<(), String> {
        if payload.contains("bad") {
            return Err("unrecognised statusline".into());
        }
        *self
            .last_statusline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(payload.to_string());
        Ok(())
    }
}

fn snapshot(state: ProviderState) -> Snapshot {
    assemble(
        1,
        1_790_596_800,
        &[ProviderSnapshot::empty(ProviderId::Claude, state)],
        &Config::default(),
    )
}

/// Serve `backend` on `bus` under the well-known name.
async fn serve(
    bus: &common::PrivateBus,
    backend: Arc<dyn Backend>,
    initial: Snapshot,
) -> (zbus::Connection, Arc<Publisher>) {
    let connection = bus.connect().await;
    let publisher = Publisher::new(initial);
    service::serve(&connection, Arc::clone(&publisher), backend)
        .await
        .expect("object served");
    publisher.attach(connection.clone());
    let reply = connection
        .request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())
        .await
        .expect("name request answered");
    assert_eq!(reply, RequestNameReply::PrimaryOwner);
    (connection, publisher)
}

async fn client(bus: &common::PrivateBus) -> (zbus::Connection, TokenStationProxy<'static>) {
    let connection = bus.connect().await;
    let proxy = token_station::dbus::client::connect(&connection)
        .await
        .expect("proxy built");
    (connection, proxy)
}

async fn next<S: Stream + Unpin>(stream: &mut S) -> Option<S::Item> {
    std::future::poll_fn(|cx| std::pin::Pin::new(&mut *stream).poll_next(cx)).await
}

#[tokio::test(flavor = "multi_thread")]
async fn properties_are_served_and_changes_are_signalled() {
    let bus = private_bus_or_skip!();
    let backend = Arc::new(FakeBackend::default());
    let (_daemon, publisher) = serve(&bus, backend, snapshot(ProviderState::Loading)).await;
    let (_client, proxy) = client(&bus).await;

    assert_eq!(proxy.revision().await.unwrap(), 1);
    let first: serde_json::Value = serde_json::from_str(&proxy.snapshot().await.unwrap()).unwrap();
    assert_eq!(first["schemaVersion"], 1);
    assert_eq!(first["providers"][0]["state"], "loading");

    let mut changes = proxy.receive_snapshot_changed().await;
    assert!(publisher.publish(snapshot(ProviderState::Ok)).await);

    // The stream replays the cached value first, so read until the new one lands.
    let value = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let property = next(&mut changes).await.expect("stream is live");
            let json = property.get().await.expect("property readable");
            let value: serde_json::Value = serde_json::from_str(&json).unwrap();
            if value["providers"][0]["state"] == "ok" {
                return value;
            }
        }
    })
    .await
    .expect("PropertiesChanged arrived");
    assert_eq!(value["revision"], 2);
    assert_eq!(proxy.revision().await.unwrap(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_refresh_calls_share_one_run() {
    let bus = private_bus_or_skip!();
    let backend = Arc::new(FakeBackend::default());
    let (_daemon, _publisher) = serve(
        &bus,
        Arc::clone(&backend) as Arc<dyn Backend>,
        snapshot(ProviderState::Ok),
    )
    .await;

    let mut clients = Vec::new();
    for _ in 0..4 {
        clients.push(client(&bus).await);
    }
    // Spawn, so all four calls really are in flight at the same time.
    let calls: Vec<_> = clients
        .iter()
        .map(|(_, proxy)| {
            let proxy = proxy.clone();
            tokio::spawn(async move { proxy.refresh().await })
        })
        .collect();
    for call in calls {
        call.await.expect("task joined").expect("Refresh returned");
    }
    assert_eq!(backend.refreshes.load(Ordering::SeqCst), 1);

    // A later call is a new run.
    clients[0].1.refresh().await.unwrap();
    assert_eq!(backend.refreshes.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn get_history_answers_json_and_rejects_unknown_windows() {
    let bus = private_bus_or_skip!();
    let (_daemon, _publisher) = serve(
        &bus,
        Arc::new(FakeBackend::default()),
        snapshot(ProviderState::Ok),
    )
    .await;
    let (_client, proxy) = client(&bus).await;

    let json = proxy.get_history("claude", "session", 1_000).await.unwrap();
    assert_eq!(json, "[[1000,10.0],[1060,20.5]]");

    for (provider, window) in [("gemini", "session"), ("claude", ""), ("claude", "nope")] {
        assert_eq!(
            proxy.get_history(provider, window, 0).await.unwrap(),
            "[]",
            "{provider}/{window}"
        );
    }
}

#[expect(
    clippy::float_cmp,
    reason = "compares exact literals that never went through arithmetic"
)]
#[tokio::test(flavor = "multi_thread")]
async fn settings_round_trip_and_invalid_input_lists_every_problem() {
    let bus = private_bus_or_skip!();
    let (_daemon, _publisher) = serve(
        &bus,
        Arc::new(FakeBackend::default()),
        snapshot(ProviderState::Ok),
    )
    .await;
    let (_client, proxy) = client(&bus).await;

    let settings = proxy.get_settings().await.unwrap();
    assert_eq!(Config::from_json(&settings).unwrap(), Config::default());

    proxy
        .set_settings(r#"{"alerts":{"warning_percent":70.0}}"#)
        .await
        .unwrap();
    let updated = Config::from_json(&proxy.get_settings().await.unwrap()).unwrap();
    assert_eq!(updated.alerts.warning_percent, 70.0);

    let error = proxy
        .set_settings(
            r#"{"general":{"limits_interval_secs":5},"alerts":{"warning_percent":99,"critical_percent":90}}"#,
        )
        .await
        .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("InvalidArgs"), "{message}");
    assert!(message.contains("limits_interval_secs"), "{message}");
    assert!(message.contains("must not exceed"), "{message}");

    assert!(proxy.set_settings("{\"bogus\":1}").await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn statusline_ingest_validates_size_and_content() {
    let bus = private_bus_or_skip!();
    let backend = Arc::new(FakeBackend::default());
    let (_daemon, _publisher) = serve(
        &bus,
        Arc::clone(&backend) as Arc<dyn Backend>,
        snapshot(ProviderState::Ok),
    )
    .await;
    let (_client, proxy) = client(&bus).await;

    proxy
        .ingest_claude_statusline(r#"{"model":{"display_name":"Opus 5.5"}}"#)
        .await
        .unwrap();
    assert!(
        backend
            .last_statusline
            .lock()
            .unwrap()
            .as_deref()
            .is_some_and(|p| p.contains("Opus"))
    );

    let oversized = "x".repeat(64 * 1024 + 1);
    let error = proxy
        .ingest_claude_statusline(&oversized)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("InvalidArgs"), "{error}");
    assert!(error.contains("65536"), "{error}");

    let rejected = proxy
        .ingest_claude_statusline("bad")
        .await
        .unwrap_err()
        .to_string();
    assert!(rejected.contains("InvalidArgs"), "{rejected}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_daemon_cannot_take_the_name() {
    let bus = private_bus_or_skip!();
    let (_daemon, _publisher) = serve(
        &bus,
        Arc::new(FakeBackend::default()),
        snapshot(ProviderState::Ok),
    )
    .await;

    let other = bus.connect().await;
    let reply = other
        .request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())
        .await;
    match reply {
        Ok(other) => assert_ne!(other, RequestNameReply::PrimaryOwner),
        Err(zbus::Error::NameTaken) => {}
        Err(other) => panic!("unexpected reply: {other}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_object_is_served_at_the_documented_path() {
    let bus = private_bus_or_skip!();
    let (_daemon, _publisher) = serve(
        &bus,
        Arc::new(FakeBackend::default()),
        snapshot(ProviderState::Ok),
    )
    .await;
    let connection = bus.connect().await;
    let introspect = zbus::fdo::IntrospectableProxy::builder(&connection)
        .destination(BUS_NAME)
        .unwrap()
        .path(OBJECT_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();
    let xml = introspect.introspect().await.unwrap();
    for member in [
        "dev.soldunov.TokenStation1",
        "\"Snapshot\"",
        "\"Revision\"",
        "\"Refresh\"",
        "\"GetHistory\"",
        "\"GetSettings\"",
        "\"SetSettings\"",
        "\"IngestClaudeStatusline\"",
    ] {
        assert!(xml.contains(member), "introspection is missing {member}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bus_name_is_only_reported_once_it_is_owned() {
    let bus = private_bus_or_skip!();
    let probe = bus.connect().await;
    assert!(!token_station::dbus::client::daemon_is_running(&probe).await);

    let (_daemon, _publisher) = serve(
        &bus,
        Arc::new(FakeBackend::default()),
        snapshot(ProviderState::Ok),
    )
    .await;
    assert!(token_station::dbus::client::daemon_is_running(&probe).await);
}
