//! The tray's D-Bus watcher against a real daemon object on a private bus.

mod common;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use token_station::backend::{Backend, SetSettingsError};
use token_station::dbus::{BUS_NAME, service};
use token_station::publish::Publisher;
use token_station::tray::client::{self, Update};
use ts_core::assemble::assemble;
use ts_core::config::Config;
use ts_core::{Level, ProviderId, ProviderSnapshot, ProviderState, Snapshot};
use zbus::fdo::{RequestNameFlags, RequestNameReply};

/// A backend that only counts refreshes.
#[derive(Default)]
struct CountingBackend {
    refreshes: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl Backend for CountingBackend {
    async fn refresh(&self) {
        self.refreshes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    async fn history(&self, _: ProviderId, _: &str, _: i64) -> Vec<(i64, f64)> {
        Vec::new()
    }

    fn settings_json(&self) -> String {
        Config::default().to_json()
    }

    async fn set_settings(&self, _: &str) -> Result<(), SetSettingsError> {
        Ok(())
    }

    async fn ingest_claude_statusline(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

fn snapshot(revision: u64, percent: f64) -> Snapshot {
    let mut claude = ProviderSnapshot::empty(ProviderId::Claude, ProviderState::Ok);
    claude.windows = vec![ts_core::UsageWindow {
        id: "session".into(),
        label: "Session".into(),
        kind: ts_core::WindowKind::Session,
        used_percent: percent,
        resets_at: Some(1_790_613_600),
        window_minutes: Some(300),
        level: Level::Normal,
        source: "oauth".into(),
        observed_at: 1_790_596_740,
    }];
    assemble(revision, 1_790_596_800, &[claude], &Config::default())
}

async fn serve(bus: &common::PrivateBus, initial: Snapshot) -> (zbus::Connection, Arc<Publisher>) {
    let connection = bus.connect().await;
    let publisher = Publisher::new(initial);
    service::serve(
        &connection,
        Arc::clone(&publisher),
        Arc::new(CountingBackend::default()),
    )
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

#[tokio::test(flavor = "multi_thread")]
async fn the_tray_reads_the_running_daemon() {
    let bus = private_bus_or_skip!();
    let (_daemon, _publisher) = serve(&bus, snapshot(1, 34.0)).await;

    let connection = bus.connect().await;
    let first = client::initial_snapshot(&connection)
        .await
        .expect("a snapshot");
    assert_eq!(first.revision, 1);
    assert_eq!(first.meter.bars[0].percent, Some(34.0));
    // And it can ask for a refresh.
    client::request_refresh(&connection).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_snapshot_reaches_the_tray() {
    let bus = private_bus_or_skip!();
    let (_daemon, publisher) = serve(&bus, snapshot(1, 34.0)).await;

    let connection = bus.connect().await;
    let (updates, mut inbox) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(client::watch_daemon(connection, updates));

    assert!(publisher.publish(snapshot(2, 91.0)).await);
    let arrived = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match inbox.recv().await.expect("the watcher is alive") {
                Update::Daemon(Some(snapshot)) if snapshot.revision == 2 => return snapshot,
                _ => {}
            }
        }
    })
    .await
    .expect("the property change arrived");
    assert_eq!(arrived.meter.bars[0].percent, Some(91.0));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tray_follows_the_daemon_off_and_back_onto_the_bus() {
    let bus = private_bus_or_skip!();
    let (daemon, publisher) = serve(&bus, snapshot(1, 34.0)).await;

    let connection = bus.connect().await;
    let (updates, mut inbox) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(client::watch_daemon(connection, updates));
    // Give the watcher a moment to subscribe before the name moves.
    tokio::time::sleep(Duration::from_millis(200)).await;

    daemon.release_name(BUS_NAME).await.expect("name released");
    await_update(&mut inbox, |update| matches!(update, Update::Daemon(None))).await;

    // A restarted daemon takes the name back and the tray re-reads it.
    assert!(publisher.publish(snapshot(2, 77.0)).await);
    daemon
        .request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())
        .await
        .expect("name taken again");
    await_update(
        &mut inbox,
        |update| matches!(update, Update::Daemon(Some(snapshot)) if snapshot.revision == 2),
    )
    .await;
}

/// Wait for the first update matching `wanted`, or fail the test.
async fn await_update(
    inbox: &mut tokio::sync::mpsc::UnboundedReceiver<Update>,
    wanted: impl Fn(&Update) -> bool,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let update = inbox.recv().await.expect("the watcher is alive");
            if wanted(&update) {
                return;
            }
        }
    })
    .await
    .expect("the expected update arrived");
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_daemon_there_is_no_snapshot() {
    let bus = private_bus_or_skip!();
    let connection = bus.connect().await;
    // Nothing owns the name and the private bus has no activation file.
    assert!(client::initial_snapshot(&connection).await.is_none());
}
