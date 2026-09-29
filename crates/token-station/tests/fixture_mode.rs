#![cfg(not(target_os = "macos"))]
//! Linux only: the session bus, the tray and the desktop integration.

//! Fixture mode served over a private bus: what front-end developers actually poke.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use token_station::backend::Backend;
use token_station::clock::system_clock;
use token_station::dbus::{BUS_NAME, service};
use token_station::fixture::FixturePlayer;
use ts_core::config::Config;
use zbus::fdo::{RequestNameFlags, RequestNameReply};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/fixtures")
        .join(name)
}

fn now() -> i64 {
    system_clock()()
}

#[expect(
    clippy::float_cmp,
    reason = "compares exact literals that never went through arithmetic"
)]
#[tokio::test(flavor = "multi_thread")]
async fn the_fixture_daemon_serves_live_countdowns() {
    let bus = private_bus_or_skip!();
    let player = FixturePlayer::load(
        &fixture("snapshot-ok.json"),
        Config::default(),
        system_clock(),
    )
    .expect("fixture loads");

    let connection = bus.connect().await;
    service::serve(
        &connection,
        player.publisher(),
        Arc::clone(&player) as Arc<dyn Backend>,
    )
    .await
    .expect("served");
    player.publisher().attach(std::sync::Arc::new(
        token_station::dbus::sink::PropertiesSink::new(connection.clone()),
    ));
    assert_eq!(
        connection
            .request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())
            .await
            .expect("name requested"),
        RequestNameReply::PrimaryOwner
    );

    let client = bus.connect().await;
    let proxy = token_station::dbus::client::connect(&client)
        .await
        .expect("proxy");

    let value: serde_json::Value = serde_json::from_str(&proxy.snapshot().await.unwrap()).unwrap();
    let generated = value["generatedAt"].as_i64().unwrap();
    assert!(
        (generated - now()).abs() <= 5,
        "generatedAt tracks the clock"
    );

    let resets_at = value["providers"][0]["windows"][0]["resetsAt"]
        .as_i64()
        .unwrap();
    assert!(
        resets_at > generated,
        "the countdown is still in the future"
    );
    assert_eq!(value["providers"][0]["state"], "ok");
    assert_eq!(value["meter"]["bars"][0]["provider"], "claude");

    // Refresh re-shifts rather than failing.
    proxy
        .refresh()
        .await
        .expect("Refresh works in fixture mode");

    let history: Vec<(i64, f64)> =
        serde_json::from_str(&proxy.get_history("claude", "session", 0).await.unwrap()).unwrap();
    assert!(!history.is_empty());
    assert!(history.iter().all(|(_, p)| (0.0..=100.0).contains(p)));
    assert_eq!(proxy.get_history("claude", "nope", 0).await.unwrap(), "[]");

    // Settings apply in memory only: there is no config file to write.
    proxy
        .set_settings(r#"{"alerts":{"warning_percent":20.0,"critical_percent":30.0}}"#)
        .await
        .expect("settings accepted");
    let settings = Config::from_json(&proxy.get_settings().await.unwrap()).unwrap();
    assert_eq!(settings.alerts.warning_percent, 20.0);

    let after: serde_json::Value = serde_json::from_str(&proxy.snapshot().await.unwrap()).unwrap();
    assert_eq!(
        after["providers"][0]["windows"][0]["level"], "critical",
        "the new thresholds are applied"
    );

    proxy
        .ingest_claude_statusline(r#"{"model":{"display_name":"Opus 5.5"}}"#)
        .await
        .expect("statusline accepted and ignored");
}

#[tokio::test(flavor = "multi_thread")]
async fn every_shipped_fixture_can_be_served() {
    for name in [
        "snapshot-ok.json",
        "snapshot-loading.json",
        "snapshot-near-limit.json",
        "snapshot-not-installed.json",
        "snapshot-stale-auth.json",
    ] {
        let player = FixturePlayer::load(&fixture(name), Config::default(), system_clock())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let snapshot = player.current();
        assert_eq!(snapshot.schema_version, 1);
        assert!((snapshot.generated_at - now()).abs() <= 5, "{name}");
        player.refresh().await;
    }
}
