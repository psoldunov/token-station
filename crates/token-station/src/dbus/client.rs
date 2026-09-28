//! Talking to a running daemon (`status`, `refresh`, `statusline`).

use crate::dbus::{BUS_NAME, INTERFACE_NAME, OBJECT_PATH};

/// Client side of `dev.soldunov.TokenStation1`.
#[zbus::proxy(
    interface = "dev.soldunov.TokenStation1",
    default_service = "dev.soldunov.TokenStation",
    default_path = "/dev/soldunov/TokenStation",
    gen_blocking = false
)]
pub trait TokenStation {
    #[zbus(property)]
    fn snapshot(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn revision(&self) -> zbus::Result<u64>;

    fn refresh(&self) -> zbus::Result<()>;

    fn get_history(&self, provider: &str, window_id: &str, since: u64) -> zbus::Result<String>;

    fn get_settings(&self) -> zbus::Result<String>;

    fn set_settings(&self, json: &str) -> zbus::Result<()>;

    fn ingest_claude_statusline(&self, json: &str) -> zbus::Result<()>;
}

/// Is a daemon currently on the bus?
pub async fn daemon_is_running(connection: &zbus::Connection) -> bool {
    match zbus::fdo::DBusProxy::new(connection).await {
        Ok(proxy) => match BUS_NAME.try_into() {
            Ok(name) => proxy.name_has_owner(name).await.unwrap_or(false),
            Err(_) => false,
        },
        Err(_) => false,
    }
}

/// A proxy for the daemon at its well-known address.
pub async fn connect(connection: &zbus::Connection) -> zbus::Result<TokenStationProxy<'static>> {
    TokenStationProxy::builder(connection)
        .destination(BUS_NAME)?
        .path(OBJECT_PATH)?
        .interface(INTERFACE_NAME)?
        .build()
        .await
}
