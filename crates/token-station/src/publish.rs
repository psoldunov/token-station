//! The published state: `Snapshot`, `Revision` and their `PropertiesChanged` signal.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use ts_core::Snapshot;
use zbus::Connection;
use zbus::zvariant::Value;

use crate::dbus::{INTERFACE_NAME, OBJECT_PATH};

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

/// Holds the current snapshot and tells the bus when it changes.
pub struct Publisher {
    state: RwLock<Published>,
    connection: OnceLock<Connection>,
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
            connection: OnceLock::new(),
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

    /// Start emitting `PropertiesChanged` on `connection`.
    pub fn attach(&self, connection: Connection) {
        let _ = self.connection.set(connection);
    }

    /// Store `next` when its content differs, bumping the revision.
    ///
    /// Returns the new revision, or `None` when nothing changed.
    pub fn store(&self, next: Snapshot) -> Option<u64> {
        let mut guard = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if content_equal(&guard.snapshot, &next) {
            return None;
        }
        let revision = guard.revision.saturating_add(1);
        let snapshot = Snapshot { revision, ..next };
        *guard = Published {
            revision,
            json: to_json(&snapshot),
            snapshot,
        };
        Some(revision)
    }

    /// Store `next` and, when it changed, tell the bus.
    pub async fn publish(&self, next: Snapshot) -> bool {
        match self.store(next) {
            None => false,
            Some(revision) => {
                self.emit(revision).await;
                true
            }
        }
    }

    async fn emit(&self, revision: u64) {
        let Some(connection) = self.connection.get() else {
            return;
        };
        let json = self.snapshot_json();
        let changed: HashMap<&str, Value<'_>> = HashMap::from([
            ("Snapshot", Value::from(json)),
            ("Revision", Value::from(revision)),
        ]);
        let invalidated: Vec<&str> = Vec::new();
        let body = (INTERFACE_NAME, changed, invalidated);
        let result = connection
            .emit_signal(
                None::<()>,
                OBJECT_PATH,
                "org.freedesktop.DBus.Properties",
                "PropertiesChanged",
                &body,
            )
            .await;
        if let Err(error) = result {
            tracing::warn!(%error, "cannot emit PropertiesChanged");
        }
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

        assert_eq!(publisher.store(snapshot(7, 60, ProviderState::Ok)), Some(2));
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

    #[tokio::test]
    async fn publish_without_a_connection_is_harmless() {
        let publisher = Publisher::new(snapshot(1, 0, ProviderState::Loading));
        assert!(publisher.publish(snapshot(1, 10, ProviderState::Ok)).await);
        assert!(!publisher.publish(snapshot(1, 20, ProviderState::Ok)).await);
    }
}
