//! The published state: the snapshot, its revision, and whoever is told when it
//! changes.
//!
//! The telling is a [`SnapshotSink`], not a bus signal: Linux attaches the D-Bus
//! emitter, macOS attaches the socket hub, and this module knows about neither.

use std::sync::{Arc, OnceLock, RwLock};

use async_trait::async_trait;
use ts_core::Snapshot;

/// What front ends currently see.
#[derive(Debug, Clone, PartialEq)]
pub struct Published {
    pub revision: u64,
    pub snapshot: Snapshot,
    pub json: String,
}

/// Do the two snapshots say the same thing, ignoring `revision` and `generatedAt`?
///
/// Those two fields move on every assembly, so comparing them would publish a new
/// revision every tick and wake every front end for nothing.
#[must_use]
pub fn content_equal(a: &Snapshot, b: &Snapshot) -> bool {
    a.schema_version == b.schema_version && a.meter == b.meter && a.providers == b.providers
}

/// Told whenever the published snapshot changes.
///
/// One per transport: `dbus::sink::PropertiesSink` emits `PropertiesChanged`,
/// and the macOS socket hub pushes `SnapshotChanged` to its subscribers.
#[async_trait]
pub trait SnapshotSink: Send + Sync {
    async fn snapshot_changed(&self, revision: u64, json: &str);
}

/// Holds the current snapshot and tells its sink when it changes.
pub struct Publisher {
    state: RwLock<Published>,
    sink: OnceLock<Arc<dyn SnapshotSink>>,
}

impl Publisher {
    /// Start at revision 1 with `initial`.
    #[must_use]
    pub fn new(initial: Snapshot) -> Arc<Publisher> {
        Arc::new(Publisher {
            state: RwLock::new(Published {
                revision: initial.revision,
                json: to_json(&initial),
                snapshot: initial,
            }),
            sink: OnceLock::new(),
        })
    }

    fn read(&self) -> Published {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn revision(&self) -> u64 {
        self.read().revision
    }

    pub fn snapshot_json(&self) -> String {
        self.read().json
    }

    pub fn snapshot(&self) -> Snapshot {
        self.read().snapshot
    }

    /// The revision and its JSON, read together.
    ///
    /// Reading them one after the other can straddle a `store`, and a
    /// `GetSnapshot` that answers with one revision's number and the next one's
    /// body is a client bug nobody can find.
    pub fn published(&self) -> (u64, String) {
        let published = self.read();
        (published.revision, published.json)
    }

    /// Start announcing changes through `sink`.
    ///
    /// Only the first sink is kept: a process serves one transport, and a second
    /// attach would be a wiring mistake rather than a second audience.
    pub fn attach(&self, sink: Arc<dyn SnapshotSink>) {
        let _ = self.sink.set(sink);
    }

    /// Store `next` when its content differs, bumping the revision.
    ///
    /// Returns the new revision *and the JSON that goes with it*, or `None`
    /// when nothing changed. The two are taken out together, under the write
    /// lock: re-reading the JSON afterwards is how a concurrent `store` pairs
    /// one revision's number with the next one's body, or announces a revision
    /// after a later one has already gone out.
    pub fn store(&self, next: Snapshot) -> Option<(u64, String)> {
        let mut guard = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if content_equal(&guard.snapshot, &next) {
            return None;
        }
        let revision = guard.revision.saturating_add(1);
        let snapshot = Snapshot { revision, ..next };
        let json = to_json(&snapshot);
        *guard = Published {
            revision,
            json: json.clone(),
            snapshot,
        };
        Some((revision, json))
    }

    /// Store `next` and, when it changed, tell the sink.
    pub async fn publish(&self, next: Snapshot) -> bool {
        match self.store(next) {
            None => false,
            Some((revision, json)) => {
                self.emit(revision, &json).await;
                true
            }
        }
    }

    /// Announce one revision and the body it was stored with.
    ///
    /// Two concurrent publishes can still reach the sink out of order — that is
    /// the sink's to sort out, and both of them do — but neither can hand it a
    /// revision number and a body that were never stored together.
    async fn emit(&self, revision: u64, json: &str) {
        let Some(sink) = self.sink.get() else {
            return;
        };
        sink.snapshot_changed(revision, json).await;
    }
}

fn to_json(snapshot: &Snapshot) -> String {
    serde_json::to_string(snapshot).unwrap_or_else(|error| {
        tracing::error!(%error, "cannot serialize snapshot");
        "{}".into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts_core::assemble::assemble;
    use ts_core::config::Config;
    use ts_core::{ProviderId, ProviderSnapshot, ProviderState};

    fn snapshot(revision: u64, now: i64, state: ProviderState) -> Snapshot {
        assemble(
            revision,
            now,
            &[ProviderSnapshot::empty(ProviderId::Claude, state)],
            &Config::default(),
        )
    }

    #[test]
    fn content_comparison_ignores_revision_and_time() {
        let a = snapshot(1, 100, ProviderState::Ok);
        let b = snapshot(99, 5_000, ProviderState::Ok);
        assert!(content_equal(&a, &b));
        assert!(!content_equal(&a, &snapshot(1, 100, ProviderState::Error)));
    }

    #[test]
    fn store_bumps_the_revision_only_on_real_change() {
        let publisher = Publisher::new(snapshot(1, 0, ProviderState::Loading));
        assert_eq!(publisher.revision(), 1);
        assert_eq!(
            publisher.store(snapshot(7, 60, ProviderState::Loading)),
            None
        );
        assert_eq!(publisher.revision(), 1);

        let (revision, json) = publisher
            .store(snapshot(7, 60, ProviderState::Ok))
            .expect("a change");
        assert_eq!(revision, 2);
        // The body handed back is the one that was stored, not a later read.
        assert_eq!(json, publisher.snapshot_json());
        assert_eq!(publisher.revision(), 2);
        assert_eq!(publisher.snapshot().generated_at, 60);
        assert_eq!(publisher.snapshot().revision, 2);
    }

    #[test]
    fn published_json_carries_the_new_revision() {
        let publisher = Publisher::new(snapshot(1, 0, ProviderState::Loading));
        publisher.store(snapshot(1, 10, ProviderState::Ok));
        let value: serde_json::Value = serde_json::from_str(&publisher.snapshot_json()).unwrap();
        assert_eq!(value["revision"], 2);
        assert_eq!(value["schemaVersion"], 1);
        assert_eq!(value["providers"][0]["state"], "ok");
    }

    /// Records what it was told, standing in for a bus or a socket.
    #[derive(Default)]
    struct RecordingSink {
        seen: std::sync::Mutex<Vec<(u64, String)>>,
    }

    #[async_trait]
    impl SnapshotSink for RecordingSink {
        async fn snapshot_changed(&self, revision: u64, json: &str) {
            self.seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((revision, json.to_string()));
        }
    }

    #[test]
    fn a_stored_revision_comes_back_with_its_own_body() {
        let publisher = Publisher::new(snapshot(1, 0, ProviderState::Loading));
        let (revision, json) = publisher
            .store(snapshot(1, 10, ProviderState::Ok))
            .expect("a change");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["revision"], revision);
    }

    #[tokio::test]
    async fn an_attached_sink_hears_every_change_and_nothing_else() {
        let publisher = Publisher::new(snapshot(1, 0, ProviderState::Loading));
        let sink = Arc::new(RecordingSink::default());
        publisher.attach(Arc::clone(&sink) as Arc<dyn SnapshotSink>);

        assert!(publisher.publish(snapshot(1, 10, ProviderState::Ok)).await);
        // Same content, later clock: nothing to announce.
        assert!(!publisher.publish(snapshot(1, 20, ProviderState::Ok)).await);
        assert!(
            publisher
                .publish(snapshot(1, 30, ProviderState::Error))
                .await
        );

        let seen = sink.seen.lock().expect("recorded");
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].0, 2);
        assert_eq!(seen[1].0, 3);
        assert!(seen[1].1.contains("\"revision\":3"), "{}", seen[1].1);
    }

    #[tokio::test]
    async fn publish_without_a_sink_is_harmless() {
        let publisher = Publisher::new(snapshot(1, 0, ProviderState::Loading));
        assert!(publisher.publish(snapshot(1, 10, ProviderState::Ok)).await);
        assert!(!publisher.publish(snapshot(1, 20, ProviderState::Ok)).await);
    }
}
