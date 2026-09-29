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

/// Run `work` on a session bus from a synchronous caller.
///
/// `setup` and `uninstall` have no runtime of their own, so this builds a
/// one-thread one for the call. `bus_address` names the bus to use; `None`
/// takes the one in the environment.
///
/// # Errors
///
/// Returns a message when the runtime cannot be built, when `bus_address` is
/// not a usable address or that bus cannot be connected to, plus whatever
/// `work` itself reports.
pub fn with_session_bus_blocking<T, F, Fut>(bus_address: Option<&str>, work: F) -> Result<T, String>
where
    F: FnOnce(zbus::Connection) -> Fut,
    Fut: std::future::Future<Output = zbus::Result<T>>,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let connection = match bus_address {
            Some(address) => zbus::connection::Builder::address(address)
                .map_err(|e| e.to_string())?
                .build()
                .await
                .map_err(|e| e.to_string())?,
            None => zbus::Connection::session()
                .await
                .map_err(|e| e.to_string())?,
        };
        work(connection).await.map_err(|e| e.to_string())
    })
}

/// A proxy for the daemon at its well-known address.
///
/// # Errors
///
/// Returns a [`zbus::Error`] when the destination, path or interface name is
/// rejected, or when the proxy cannot be built on `connection`.
pub async fn connect(connection: &zbus::Connection) -> zbus::Result<TokenStationProxy<'static>> {
    TokenStationProxy::builder(connection)
        .destination(BUS_NAME)?
        .path(OBJECT_PATH)?
        .interface(INTERFACE_NAME)?
        .build()
        .await
}
