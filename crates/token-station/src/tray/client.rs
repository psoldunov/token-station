//! The tray's half of the D-Bus conversation.
//!
//! Two watchers feed the tray: one follows the daemon's `Snapshot` property and
//! the ownership of `dev.soldunov.TokenStation`, the other follows the portal's
//! colour scheme. Both push into the same channel so the tray applies them in
//! one place.

use futures_util::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use ts_core::Snapshot;

use crate::dbus::client::{connect, daemon_is_running, with_session_bus_blocking};
use crate::dbus::{BUS_NAME, INTERFACE_NAME, OBJECT_PATH};
use crate::tray::palette::ColorScheme;
use crate::tray::portal::{self, SettingsProxy};
use crate::tray::{TRAY_BUS_NAME, TRAY_INTERFACE_NAME, TRAY_OBJECT_PATH};

/// One thing the tray has to react to.
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    /// A new snapshot, or `None` when the daemon left the bus.
    Daemon(Option<Box<Snapshot>>),
    /// The panel switched between light and dark.
    Scheme(ColorScheme),
}

/// Parse a published snapshot, logging (not propagating) a malformed one.
fn parse(json: &str) -> Option<Box<Snapshot>> {
    match serde_json::from_str::<Snapshot>(json) {
        Ok(snapshot) => Some(Box::new(snapshot)),
        Err(error) => {
            tracing::warn!(%error, "the daemon sent an unreadable snapshot");
            None
        }
    }
}

/// Read the current snapshot with a plain `Get` call.
///
/// The typed proxy caches properties, and that cache survives a daemon restart,
/// so every re-read after an ownership change has to go around it.
async fn fetch(connection: &zbus::Connection) -> Option<Box<Snapshot>> {
    let proxy = zbus::fdo::PropertiesProxy::builder(connection)
        .destination(BUS_NAME)
        .ok()?
        .path(OBJECT_PATH)
        .ok()?
        .build()
        .await
        .ok()?;
    let interface = INTERFACE_NAME.try_into().ok()?;
    match proxy.get(interface, "Snapshot").await {
        Ok(value) => parse(&String::try_from(value).ok()?),
        Err(error) => {
            tracing::debug!(%error, "no snapshot from the daemon");
            None
        }
    }
}

/// First reading. When nothing owns the name, one `Refresh` call triggers D-Bus
/// activation; if that also fails the tray shows its "not running" state.
pub async fn initial_snapshot(connection: &zbus::Connection) -> Option<Box<Snapshot>> {
    let proxy = connect(connection).await.ok()?;
    if !daemon_is_running(connection).await {
        if let Err(error) = proxy.refresh().await {
            tracing::info!(%error, "the daemon is not running and could not be activated");
            return None;
        }
    }
    fetch(connection).await
}

/// Ask the daemon to refresh now.
///
/// # Errors
///
/// Returns a [`zbus::Error`] when the daemon proxy cannot be built or the
/// `Refresh` call fails.
pub async fn request_refresh(connection: &zbus::Connection) -> zbus::Result<()> {
    connect(connection).await?.refresh().await
}

/// Ask a running tray to quit. `Ok(false)` when no tray owns the name.
///
/// # Errors
///
/// Returns a [`zbus::Error`] when the bus daemon cannot be queried for the name
/// owner, or when the `Quit` call to the running tray fails.
pub async fn request_quit(connection: &zbus::Connection) -> zbus::Result<bool> {
    let bus = zbus::fdo::DBusProxy::new(connection).await?;
    if !bus.name_has_owner(TRAY_BUS_NAME.try_into()?).await? {
        return Ok(false);
    }
    let proxy = zbus::Proxy::new(
        connection,
        TRAY_BUS_NAME,
        TRAY_OBJECT_PATH,
        TRAY_INTERFACE_NAME,
    )
    .await?;
    proxy.call_method("Quit", &()).await?;
    Ok(true)
}

/// [`request_quit`] from a synchronous caller: `uninstall` has no runtime.
///
/// `bus_address` names the bus to use; `None` takes the one in the environment.
///
/// # Errors
///
/// Returns a message when the bus cannot be reached (see
/// [`with_session_bus_blocking`]), plus whatever [`request_quit`] itself reports.
pub fn request_quit_blocking(bus_address: Option<&str>) -> Result<bool, String> {
    with_session_bus_blocking(bus_address, |connection| async move {
        request_quit(&connection).await
    })
}

/// Follow `Snapshot` and the ownership of the daemon's bus name until the
/// channel closes.
pub async fn watch_daemon(connection: zbus::Connection, updates: UnboundedSender<Update>) {
    let Ok(proxy) = connect(&connection).await else {
        return;
    };
    let mut snapshots = proxy
        .inner()
        .receive_property_changed::<String>("Snapshot")
        .await;
    let Ok(mut owners) = proxy.inner().receive_owner_changed().await else {
        return;
    };
    loop {
        let update = tokio::select! {
            Some(changed) = snapshots.next() => match changed.get().await {
                Ok(json) => Update::Daemon(parse(&json)),
                Err(_) => continue,
            },
            Some(owner) = owners.next() => match owner {
                // A new owner appeared: re-read rather than trust the cache.
                Some(_) => Update::Daemon(fetch(&connection).await),
                None => Update::Daemon(None),
            },
            else => break,
        };
        if updates.send(update).is_err() {
            break;
        }
    }
}

/// Follow `SettingChanged` for the appearance colour scheme.
pub async fn watch_scheme(connection: zbus::Connection, updates: UnboundedSender<Update>) {
    let Ok(proxy) = SettingsProxy::new(&connection).await else {
        return;
    };
    let Ok(mut changes) = proxy.receive_setting_changed().await else {
        return;
    };
    while let Some(signal) = changes.next().await {
        let Ok(args) = signal.args() else { continue };
        if !portal::is_color_scheme(args.namespace(), args.key()) {
            continue;
        }
        let Some(scheme) = portal::scheme_from_value(args.value()) else {
            continue;
        };
        if updates.send(Update::Scheme(scheme)).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_published_snapshot_round_trips() {
        let json = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../data/fixtures/snapshot-ok.json"
        ))
        .expect("fixture reads");
        let parsed = parse(&json).expect("fixture parses");
        assert_eq!(parsed.schema_version, ts_core::snapshot::SCHEMA_VERSION);
        assert_eq!(parsed.meter.bars.len(), 2);
    }

    #[test]
    fn a_malformed_snapshot_is_dropped_not_panicked_on() {
        assert!(parse("").is_none());
        assert!(parse("{}").is_none());
        assert!(parse(r#"{"schemaVersion":1}"#).is_none());
    }
}
