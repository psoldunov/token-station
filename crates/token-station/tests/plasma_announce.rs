#![cfg(not(target_os = "macos"))]
//! Linux only: the session bus, the tray and the desktop integration.

//! `setup` and `uninstall` tell a running plasmashell about the applet, on a
//! private bus standing in for the session.
//!
//! Plasma's system tray lists applets once, when plasmashell starts. After that
//! it only adds or drops one when `KPackage` announces the change on the session
//! bus, so a copy that is not announced stays invisible until the next login.

mod common;

use std::path::Path;
use std::time::Duration;

use futures_util::StreamExt;
use token_station::integration::existing::SystemDirs;
use token_station::integration::plasma::{APPLET_ID, KPACKAGE_INTERFACE, KPACKAGE_PATH};
use token_station::integration::{self, DesktopChoice, Session, SetupOptions};
use token_station::paths::Env;

/// How long to wait for a signal that should already be on the bus.
const PATIENCE: Duration = Duration::from_secs(5);

/// A listener for every `KPackage` applet signal, as the tray subscribes to them.
async fn kpackage_signals(bus: &common::PrivateBus) -> zbus::MessageStream {
    let connection = bus.connect().await;
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .path(KPACKAGE_PATH)
        .expect("valid object path")
        .interface(KPACKAGE_INTERFACE)
        .expect("valid interface name")
        .build();
    zbus::MessageStream::for_match_rule(rule, &connection, None)
        .await
        .expect("match rule accepted")
}

/// The next signal's member and its one string argument.
async fn next_signal(signals: &mut zbus::MessageStream) -> (String, String) {
    let message = tokio::time::timeout(PATIENCE, signals.next())
        .await
        .expect("a signal arrived in time")
        .expect("the stream is open")
        .expect("a well-formed message");
    let member = message
        .header()
        .member()
        .expect("signals carry a member")
        .to_string();
    let applet: String = message.body().deserialize().expect("one string argument");
    (member, applet)
}

fn env_for(root: &Path) -> Env {
    let at = |suffix: &str| Some(root.join(suffix).to_string_lossy().into_owned());
    Env {
        socket: None,
        home: at("home"),
        config_home: at("config"),
        state_home: at("state"),
        cache_home: at("cache"),
        runtime_dir: at("run"),
        data_home: at("data"),
    }
}

/// Collect the next `count` `KPackage` signals on `bus` while `during` runs.
///
/// The listener lives on its own thread with its own runtime, so every zbus
/// object is created and dropped inside it, and `setup`, which builds a runtime
/// of its own, never runs inside another.
fn signals_during<T>(
    bus: &common::PrivateBus,
    count: usize,
    during: impl FnOnce() -> T,
) -> (T, Vec<(String, String)>) {
    std::thread::scope(|scope| {
        let (ready, listening) = std::sync::mpsc::channel();
        let listener = scope.spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async {
                let mut signals = kpackage_signals(bus).await;
                ready.send(()).expect("the test is waiting");
                let mut received = Vec::new();
                for _ in 0..count {
                    received.push(next_signal(&mut signals).await);
                }
                received
            })
        });
        listening.recv().expect("the listener subscribed");
        let result = during();
        (result, listener.join().expect("the listener finished"))
    })
}

#[test]
fn setup_and_uninstall_announce_the_applet_to_a_running_plasma() {
    let Some(bus) = common::PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let root = tempfile::tempdir().expect("temp dir");
    let payload = root.path().join("integrations/plasma");
    std::fs::create_dir_all(&payload).unwrap();
    std::fs::write(payload.join("metadata.json"), "{}").unwrap();
    let env = env_for(root.path());
    let session = Session {
        current_desktop: Some("KDE".into()),
        appimage: Some("/apps/TokenStation-x86_64.AppImage".into()),
        // No `systemctl` on this PATH: the unit is written but never enabled.
        path: Some(root.path().join("bin").to_string_lossy().into_owned()),
        bus_address: Some(bus.address.clone()),
        system: SystemDirs::default(),
        ..Session::default()
    };
    let options = SetupOptions {
        desktop: DesktopChoice::Auto,
        payload_dir: Some(root.path().join("integrations")),
        ..SetupOptions::default()
    };

    let ((installed, uninstalled), signals) = signals_during(&bus, 2, || {
        let installed = integration::setup(&options, &env, &session, 0).expect("setup succeeds");
        let uninstalled = integration::uninstall(&env, &session).expect("uninstall succeeds");
        (installed, uninstalled)
    });

    for summary in [installed, uninstalled] {
        assert!(
            !summary.contains("next login"),
            "announced cleanly:\n{summary}"
        );
    }
    assert_eq!(
        signals,
        vec![
            ("packageInstalled".to_string(), APPLET_ID.to_string()),
            ("packageUninstalled".to_string(), APPLET_ID.to_string()),
        ]
    );
}
