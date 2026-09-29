//! Reaching a running daemon from the CLI.
//!
//! `status`, `refresh` and `statusline` all want the same three things from
//! whatever daemon is running, and the transport under them is whatever the
//! platform has: the session bus on Linux, the Unix socket on macOS. The rest of
//! the CLI is written against this module and never learns which.

use std::time::Duration;

use crate::paths::Paths;

/// Message used when a command needs a daemon and there is none.
pub const NO_DAEMON: &str = "no token-station daemon is running";

/// How long a daemon gets to hand back state it already holds.
///
/// Generous for a lock read; the point is that a wedged daemon makes
/// `token-station status` fall back to reading the providers itself rather than
/// wait for a process that is never going to answer.
pub const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a `Refresh` gets. It talks to two upstream APIs and to the `codex`
/// binary, so it is measured in tens of seconds, not in milliseconds.
pub const REFRESH_TIMEOUT: Duration = Duration::from_secs(60);

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::{Duration, Paths};
    use anyhow::Context;

    use crate::dbus::client;

    /// The running daemon's snapshot JSON, or `None` when none is running.
    pub async fn snapshot_json(_paths: &Paths) -> Option<anyhow::Result<String>> {
        let connection = zbus::Connection::session().await.ok()?;
        if !client::daemon_is_running(&connection).await {
            return None;
        }
        Some(fetch(connection).await)
    }

    async fn fetch(connection: zbus::Connection) -> anyhow::Result<String> {
        let proxy = client::connect(&connection).await?;
        Ok(proxy.snapshot().await?)
    }

    /// Ask the daemon to refresh now.
    ///
    /// # Errors
    ///
    /// Returns an error when the session bus is unreachable, when no daemon
    /// owns the well-known name, or when the call itself fails.
    pub async fn refresh(_paths: &Paths) -> anyhow::Result<()> {
        let connection = zbus::Connection::session()
            .await
            .context("cannot connect to the session bus")?;
        anyhow::ensure!(
            client::daemon_is_running(&connection).await,
            super::NO_DAEMON
        );
        client::connect(&connection).await?.refresh().await?;
        Ok(())
    }

    /// Hand a statusline payload to the daemon, within `timeout`.
    ///
    /// # Errors
    ///
    /// Returns a message when the bus is unreachable, when the call does not
    /// answer within `timeout`, or when the daemon rejects the payload.
    pub async fn ingest_statusline(
        _paths: &Paths,
        payload: &str,
        timeout: Duration,
    ) -> Result<(), String> {
        let connection = zbus::Connection::session()
            .await
            .map_err(|e| e.to_string())?;
        let proxy = client::connect(&connection)
            .await
            .map_err(|e| e.to_string())?;
        tokio::time::timeout(timeout, proxy.ingest_claude_statusline(payload))
            .await
            .map_err(|_| "timed out".to_string())?
            .map_err(|e| e.to_string())
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{Duration, Paths};
    use anyhow::Context;
    use serde_json::json;

    use crate::ipc::client::{CONNECT_TIMEOUT, CallError, Client};

    /// The running daemon's snapshot JSON, or `None` when none is listening.
    ///
    /// Nothing on the socket is not an error: `status` answers by reading the
    /// providers itself, exactly as it does on Linux without a bus name.
    pub async fn snapshot_json(paths: &Paths) -> Option<anyhow::Result<String>> {
        let mut client = Client::connect(&paths.socket_path(), CONNECT_TIMEOUT)
            .await
            .ok()?;
        Some(fetch(&mut client).await)
    }

    async fn fetch(client: &mut Client) -> anyhow::Result<String> {
        // Connecting is not the same as being answered: a daemon wedged after
        // the accept still takes the connection.
        let result = tokio::time::timeout(super::READ_TIMEOUT, client.call("GetSnapshot", None))
            .await
            .map_err(|_| anyhow::anyhow!("the daemon did not answer GetSnapshot in time"))?
            .context("the daemon refused GetSnapshot")?;
        let snapshot = result
            .get("snapshot")
            .ok_or_else(|| anyhow::anyhow!("the daemon sent a reply without a snapshot"))?;
        Ok(serde_json::to_string(snapshot)?)
    }

    /// Ask the daemon to refresh now.
    ///
    /// # Errors
    ///
    /// Returns an error when nothing is listening on the socket, or when the
    /// call itself fails.
    pub async fn refresh(paths: &Paths) -> anyhow::Result<()> {
        let mut client = Client::connect(&paths.socket_path(), CONNECT_TIMEOUT)
            .await
            .map_err(|_| anyhow::anyhow!(super::NO_DAEMON))?;
        tokio::time::timeout(super::REFRESH_TIMEOUT, client.call("Refresh", None))
            .await
            .map_err(|_| anyhow::anyhow!("the daemon did not finish the refresh in time"))?
            .context("the daemon refused Refresh")?;
        Ok(())
    }

    /// Hand a statusline payload to the daemon, within `timeout`.
    ///
    /// # Errors
    ///
    /// Returns a message when nothing is listening, when the exchange does not
    /// finish within `timeout`, or when the daemon rejects the payload.
    pub async fn ingest_statusline(
        paths: &Paths,
        payload: &str,
        timeout: Duration,
    ) -> Result<(), String> {
        // The whole exchange is bounded, not just the call: Claude Code runs the
        // statusline hook on every turn.
        let work = async {
            let mut client = Client::connect(&paths.socket_path(), timeout).await?;
            client
                .call("IngestClaudeStatusline", Some(json!({"json": payload})))
                .await
                .map_err(|error| match error {
                    CallError::Io(error) => error,
                    other => std::io::Error::other(other.to_string()),
                })?;
            Ok::<(), std::io::Error>(())
        };
        tokio::time::timeout(timeout, work)
            .await
            .map_err(|_| "timed out".to_string())?
            .map_err(|error| error.to_string())
    }
}

pub use imp::{ingest_statusline, refresh, snapshot_json};
