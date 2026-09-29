//! Alerts reach a notification server from inside the daemon's own runtime.
//!
//! This is the regression test for the panic a convenience wrapper caused: those
//! build a runtime of their own, and doing that on a tokio worker aborts the task
//! with "Cannot start a runtime from within a runtime" — so the very first alert
//! after a threshold crossing killed the refresh.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use token_station::notify::{self, DesktopNotifier, NOTIFICATIONS_PATH, NOTIFICATIONS_SERVICE};
use ts_core::alerts::{Alert, AlertKind};
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::message::Type;
use zbus::zvariant::OwnedValue;

/// One recorded `Notify` call, in the order the interface declares.
type Call = (
    String,
    u32,
    String,
    String,
    String,
    Vec<String>,
    HashMap<String, OwnedValue>,
    i32,
);

/// Answer `Notify` on the private bus and record what arrived.
///
/// Written against the raw message stream rather than a generated interface, so
/// the test asserts the wire call the daemon actually makes.
async fn serve_fake_server(connection: zbus::Connection, calls: Arc<Mutex<Vec<Call>>>) {
    let mut messages = zbus::MessageStream::from(&connection);
    while let Some(Ok(message)) = messages.next().await {
        if message.message_type() != Type::MethodCall {
            continue;
        }
        let header = message.header();
        if header.member().is_none_or(|m| m.as_str() != "Notify") {
            continue;
        }
        match message.body().deserialize::<Call>() {
            Ok(call) => {
                calls.lock().expect("no panic held the lock").push(call);
                let _ = connection.reply(&message.header(), &(1u32,)).await;
            }
            Err(error) => panic!("the daemon sent an unexpected Notify body: {error}"),
        }
    }
}

/// Start the stand-in server and hand back what it collects.
async fn fake_server(bus: &common::PrivateBus) -> Arc<Mutex<Vec<Call>>> {
    let connection = bus.connect().await;
    connection
        .object_server()
        .at(NOTIFICATIONS_PATH, Placeholder)
        .await
        .expect("the object path is served");
    assert_eq!(
        connection
            .request_name_with_flags(NOTIFICATIONS_SERVICE, RequestNameFlags::DoNotQueue.into())
            .await
            .expect("the name is free on a private bus"),
        RequestNameReply::PrimaryOwner
    );

    let calls = Arc::new(Mutex::new(Vec::new()));
    tokio::spawn(serve_fake_server(connection, Arc::clone(&calls)));
    calls
}

/// Only there so the path exists; `Notify` is answered off the message stream.
struct Placeholder;

#[zbus::interface(name = "dev.soldunov.TokenStationTest")]
impl Placeholder {
    #[expect(
        clippy::unused_self,
        reason = "zbus requires an interface method to take &self"
    )]
    fn ping(&self) {}
}

fn alert(kind: AlertKind, percent: f64) -> Alert {
    Alert {
        provider: ts_core::ProviderId::Claude,
        window_id: "session".into(),
        label: "Session".into(),
        kind,
        percent,
        resets_at: Some(8_040),
    }
}

/// Wait for `count` calls, or give up after a second.
async fn settle(calls: &Mutex<Vec<Call>>, count: usize) -> Vec<Call> {
    for _ in 0..200 {
        {
            let seen = calls.lock().expect("no panic held the lock");
            if seen.len() >= count {
                return seen.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    calls.lock().expect("no panic held the lock").clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_alert_raised_on_a_worker_thread_reaches_the_server() {
    let bus = private_bus_or_skip!();
    let calls = fake_server(&bus).await;

    let notifier = Arc::new(DesktopNotifier::new(bus.connect().await));
    // Spawned, so the notification is sent from a tokio worker: that is exactly
    // where a nested runtime used to panic.
    let sender = tokio::spawn(async move {
        notify::deliver(
            notifier.as_ref(),
            &[
                alert(AlertKind::Warning, 82.0),
                alert(AlertKind::Critical, 96.0),
            ],
            0,
        )
        .await;
    });
    sender.await.expect("no task panicked while notifying");

    let seen = settle(&calls, 2).await;
    assert_eq!(seen.len(), 2, "both alerts arrived");
    let (app_name, _, icon, summary, body, actions, hints, expire) =
        seen.first().expect("a first call").clone();
    assert_eq!(app_name, notify::APP_NAME);
    assert_eq!(icon, notify::APP_ID);
    assert_eq!(summary, "Claude Code: Session at 82 %");
    assert_eq!(body, "Resets in 2 h 14 min");
    assert!(actions.is_empty());
    assert_eq!(expire, -1);
    assert_eq!(u8::try_from(&hints["urgency"]).expect("a byte"), 1);
    assert_eq!(
        hints["desktop-entry"]
            .downcast_ref::<&str>()
            .expect("a string"),
        notify::APP_ID
    );

    let critical = seen.get(1).expect("a second call");
    assert_eq!(
        u8::try_from(&critical.6["urgency"]).expect("a byte"),
        2,
        "a critical alert is urgent"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_notification_server_that_is_not_there_is_only_a_warning() {
    let bus = private_bus_or_skip!();
    // No server on this bus at all. The bus spends its own activation timeout
    // before answering, so the wait is bounded here: the point is that a failed
    // call is logged and returns, rather than taking the refresh down with it.
    let notifier = DesktopNotifier::new(bus.connect().await);
    let _ = tokio::time::timeout(
        Duration::from_secs(2),
        notify::deliver(&notifier, &[alert(AlertKind::Warning, 80.0)], 0),
    )
    .await;
}
