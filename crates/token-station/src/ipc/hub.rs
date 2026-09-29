//! What the socket server pushes to its subscribers.
//!
//! The hub is the daemon's [`SnapshotSink`] and its [`Notifier`] at once, which
//! is the whole of the difference between the two platforms: on Linux a
//! threshold crossing becomes a `Notify` call on the session bus, and here it
//! becomes an `Alert` notification the menu bar app shows itself.
//!
//! Snapshots are latest-wins and alerts are not. A client that reads slowly may
//! skip a revision, because only the newest one says anything true; skipping an
//! alert would lose the one thing the user asked to be told about, so alerts
//! raised while nobody is listening are kept for the next subscriber.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::{broadcast, watch};
use ts_core::alerts::Alert;

use crate::ipc::protocol;
use crate::notify::Notifier;
use crate::publish::SnapshotSink;

/// How many alerts are kept while nobody is subscribed.
pub const ALERT_BACKLOG: usize = 16;

/// How many alert lines one subscriber may fall behind before it loses the
/// oldest. Generous: alerts arrive in ones, not in floods.
const ALERT_CHANNEL: usize = 64;

/// The current snapshot, ready to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotState {
    pub revision: u64,
    pub json: String,
}

/// Alerts waiting for somebody to hear them, and how many are listening.
#[derive(Debug, Default)]
struct Backlog {
    waiting: VecDeque<String>,
    subscribers: usize,
}

/// Fans the published state and the raised alerts out to socket subscribers.
pub struct Hub {
    snapshot: watch::Sender<Arc<SnapshotState>>,
    alerts: broadcast::Sender<String>,
    backlog: Mutex<Backlog>,
}

impl Hub {
    /// A hub with no state yet; [`Hub::seed`] gives it the publisher's.
    #[must_use]
    pub fn new() -> Arc<Hub> {
        let (snapshot, _) = watch::channel(Arc::new(SnapshotState {
            revision: 0,
            json: "{}".into(),
        }));
        Arc::new(Hub {
            snapshot,
            alerts: broadcast::channel(ALERT_CHANNEL).0,
            backlog: Mutex::new(Backlog::default()),
        })
    }

    /// Set the state without announcing it, before anybody can subscribe.
    pub fn seed(&self, revision: u64, json: String) {
        self.snapshot
            .send_replace(Arc::new(SnapshotState { revision, json }));
    }

    /// The state a new subscriber starts from.
    #[must_use]
    pub fn current(&self) -> Arc<SnapshotState> {
        Arc::clone(&self.snapshot.borrow())
    }

    /// Start listening: the alerts nobody heard, then everything that follows.
    #[must_use]
    pub fn subscribe(self: &Arc<Hub>) -> Subscription {
        // The receiver is created while the lock is held, so an alert raised
        // right now is either in the backlog this takes or in the channel this
        // listens to, and never in neither.
        let mut backlog = lock(&self.backlog);
        let alerts = self.alerts.subscribe();
        backlog.subscribers += 1;
        let missed = std::mem::take(&mut backlog.waiting).into_iter().collect();
        drop(backlog);
        Subscription {
            hub: Arc::clone(self),
            snapshots: self.snapshot.subscribe(),
            alerts,
            missed,
        }
    }

    fn release(&self) {
        let mut backlog = lock(&self.backlog);
        backlog.subscribers = backlog.subscribers.saturating_sub(1);
    }

    /// Hand one alert line to every subscriber, or keep it for the next one.
    ///
    /// The send happens under the lock, and a failed send falls back to the
    /// backlog: the last subscriber going away between the count and the send
    /// would otherwise drop the alert on the floor, which is the one thing the
    /// user asked to be told about.
    fn fan_out(&self, line: String) {
        let mut backlog = lock(&self.backlog);
        if backlog.subscribers > 0 {
            match self.alerts.send(line) {
                Ok(_) => return,
                Err(tokio::sync::broadcast::error::SendError(returned)) => {
                    backlog.subscribers = 0;
                    keep(&mut backlog, returned);
                    return;
                }
            }
        }
        keep(&mut backlog, line);
    }
}

#[async_trait]
impl SnapshotSink for Hub {
    async fn snapshot_changed(&self, revision: u64, json: &str) {
        // Two publishes can reach here out of order; only the newer one is
        // worth telling anybody about, and an older one replacing it would send
        // the subscribers backwards.
        self.snapshot.send_if_modified(|current| {
            if revision <= current.revision {
                return false;
            }
            *current = Arc::new(SnapshotState {
                revision,
                json: json.to_string(),
            });
            true
        });
    }
}

#[async_trait]
impl Notifier for Hub {
    async fn notify(&self, alert: &Alert, now: i64) {
        self.fan_out(protocol::alert_notification(alert, now));
    }
}

/// One connection's view of the hub; releases its slot when it is dropped.
pub struct Subscription {
    hub: Arc<Hub>,
    /// Snapshot states, latest-wins.
    pub snapshots: watch::Receiver<Arc<SnapshotState>>,
    /// Alert lines, already serialized.
    pub alerts: broadcast::Receiver<String>,
    /// Alert lines raised while nobody was subscribed.
    pub missed: Vec<String>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.hub.release();
    }
}

/// Put one alert line in the backlog, dropping the oldest when it is full.
fn keep(backlog: &mut Backlog, line: String) {
    if backlog.waiting.len() == ALERT_BACKLOG {
        backlog.waiting.pop_front();
    }
    backlog.waiting.push_back(line);
}

fn lock(backlog: &Mutex<Backlog>) -> std::sync::MutexGuard<'_, Backlog> {
    backlog
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts_core::ProviderId;
    use ts_core::alerts::AlertKind;

    fn seeded(revision: u64, json: &str) -> Arc<Hub> {
        let hub = Hub::new();
        hub.seed(revision, json.to_string());
        hub
    }

    fn alert(percent: f64) -> Alert {
        Alert {
            provider: ProviderId::Claude,
            window_id: "session".into(),
            label: "Session".into(),
            kind: AlertKind::Warning,
            percent,
            resets_at: None,
        }
    }

    #[tokio::test]
    async fn alerts_raised_with_nobody_listening_reach_the_next_subscriber() {
        let hub = seeded(1, r#"{"revision":1}"#);
        hub.notify(&alert(81.0), 0).await;
        hub.notify(&alert(91.0), 0).await;

        let subscription = hub.subscribe();
        assert_eq!(subscription.missed.len(), 2);
        assert!(subscription.missed[0].contains("81 %"));

        // Taken, not copied: a second subscriber does not see them again.
        let second = hub.subscribe();
        assert!(second.missed.is_empty());
    }

    #[tokio::test]
    async fn the_backlog_keeps_the_newest_sixteen() {
        let hub = seeded(1, "{}");
        for index in 0..20 {
            hub.notify(&alert(f64::from(index)), 0).await;
        }
        let subscription = hub.subscribe();
        assert_eq!(subscription.missed.len(), ALERT_BACKLOG);
        // The first four are the ones that fell off the front.
        assert!(subscription.missed[0].contains("\"percent\":4"));
        assert!(subscription.missed[ALERT_BACKLOG - 1].contains("\"percent\":19"));
    }

    #[tokio::test]
    async fn a_live_subscriber_is_sent_alerts_instead_of_buffering_them() {
        let hub = seeded(1, "{}");
        let mut subscription = hub.subscribe();
        hub.notify(&alert(81.0), 0).await;

        let line = subscription.alerts.recv().await.expect("an alert");
        assert!(line.contains("\"method\":\"Alert\""), "{line}");
        assert!(lock(&hub.backlog).waiting.is_empty());
    }

    #[tokio::test]
    async fn dropping_the_last_subscriber_starts_buffering_again() {
        let hub = seeded(1, "{}");
        drop(hub.subscribe());
        hub.notify(&alert(81.0), 0).await;
        assert_eq!(hub.subscribe().missed.len(), 1);
    }

    #[tokio::test]
    async fn an_alert_is_kept_when_the_last_subscriber_left_unnoticed() {
        let hub = seeded(1, "{}");
        // The state the race leaves behind: counted as subscribed, with no
        // receiver left to hear the send. The alert has to land in the backlog
        // rather than vanish, and the count has to correct itself.
        lock(&hub.backlog).subscribers = 1;

        hub.notify(&alert(81.0), 0).await;
        assert_eq!(lock(&hub.backlog).waiting.len(), 1);
        assert_eq!(lock(&hub.backlog).subscribers, 0);
        assert_eq!(hub.subscribe().missed.len(), 1);
    }

    #[tokio::test]
    async fn an_older_revision_never_replaces_a_newer_one() {
        let hub = seeded(1, r#"{"revision":1}"#);
        hub.snapshot_changed(3, r#"{"revision":3}"#).await;
        // Out of order, and a repeat: neither may move the state backwards.
        hub.snapshot_changed(2, r#"{"revision":2}"#).await;
        hub.snapshot_changed(3, r#"{"revision":3,"stale":true}"#)
            .await;
        assert_eq!(hub.current().revision, 3);
        assert_eq!(hub.current().json, r#"{"revision":3}"#);
    }

    #[tokio::test]
    async fn snapshots_are_latest_wins() {
        let hub = seeded(1, r#"{"revision":1}"#);
        let mut subscription = hub.subscribe();
        hub.snapshot_changed(2, r#"{"revision":2}"#).await;
        hub.snapshot_changed(3, r#"{"revision":3}"#).await;

        subscription.snapshots.changed().await.expect("a change");
        // Only the newest one is left to read: a slow reader skips the middle.
        assert_eq!(subscription.snapshots.borrow_and_update().revision, 3);
        assert_eq!(hub.current().revision, 3);
    }
}
