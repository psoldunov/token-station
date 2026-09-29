//! The D-Bus side of [`crate::publish`]: `PropertiesChanged` for the two
//! properties front ends watch.

use async_trait::async_trait;
use std::collections::HashMap;
use zbus::Connection;
use zbus::zvariant::Value;

use crate::dbus::{INTERFACE_NAME, OBJECT_PATH};
use crate::publish::SnapshotSink;

/// Emits `org.freedesktop.DBus.Properties.PropertiesChanged` on the connection
/// the daemon already owns.
pub struct PropertiesSink {
    connection: Connection,
}

impl PropertiesSink {
    #[must_use]
    pub fn new(connection: Connection) -> PropertiesSink {
        PropertiesSink { connection }
    }
}

#[async_trait]
impl SnapshotSink for PropertiesSink {
    async fn snapshot_changed(&self, revision: u64, json: &str) {
        let changed: HashMap<&str, Value<'_>> = HashMap::from([
            ("Snapshot", Value::from(json)),
            ("Revision", Value::from(revision)),
        ]);
        let invalidated: Vec<&str> = Vec::new();
        let body = (INTERFACE_NAME, changed, invalidated);
        let result = self
            .connection
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
