//! The `dev.soldunov.TokenStation1` interface implementation.
//!
//! An adapter over [`Api`]: it does the zbus plumbing and the mapping from
//! [`SetSettingsError`] to a D-Bus error name, and nothing else.

use std::sync::Arc;

use zbus::fdo;

use crate::api::Api;
use crate::backend::Backend;
use crate::dbus::OBJECT_PATH;
use crate::publish::Publisher;

/// Serves the published snapshot and forwards calls to a [`Backend`].
pub struct TokenStation1 {
    api: Api,
}

impl TokenStation1 {
    pub fn new(publisher: Arc<Publisher>, backend: Arc<dyn Backend>) -> TokenStation1 {
        TokenStation1 {
            api: Api::new(publisher, backend),
        }
    }
}

#[zbus::interface(name = "dev.soldunov.TokenStation1")]
impl TokenStation1 {
    /// Full state as JSON.
    #[zbus(property)]
    fn snapshot(&self) -> String {
        self.api.snapshot_json()
    }

    /// Increments with every new snapshot.
    #[zbus(property)]
    fn revision(&self) -> u64 {
        self.api.revision()
    }

    /// Refresh every provider now; concurrent callers share one run.
    async fn refresh(&self) {
        self.api.refresh().await;
    }

    /// Recorded samples for one window since `since` (Unix seconds).
    async fn get_history(&self, provider: &str, window_id: &str, since: u64) -> String {
        self.api.history_json(provider, window_id, since).await
    }

    /// Current config as JSON.
    fn get_settings(&self) -> String {
        self.api.settings_json()
    }

    /// Replace the config; invalid input lists every problem.
    async fn set_settings(&self, json: &str) -> fdo::Result<()> {
        self.api.set_settings(json).await.map_err(as_dbus_error)
    }

    /// Raw statusline JSON piped by `token-station statusline`.
    async fn ingest_claude_statusline(&self, json: &str) -> fdo::Result<()> {
        self.api
            .ingest_claude_statusline(json)
            .await
            .map_err(fdo::Error::InvalidArgs)
    }
}

/// Which D-Bus error a failed `SetSettings` becomes.
///
/// `InvalidArgs` is a statement about what the caller sent, so it is reserved for
/// documents the daemon refused; a write that failed is reported as `Failed`,
/// which is what a client needs to tell "fix your input" from "try again".
fn as_dbus_error(error: crate::backend::SetSettingsError) -> fdo::Error {
    use crate::backend::SetSettingsError;
    match error {
        SetSettingsError::Rejected(problems) => fdo::Error::InvalidArgs(problems.join("; ")),
        SetSettingsError::Failed(message) => fdo::Error::Failed(message),
    }
}

/// Publish the object, then ask for the well-known name.
///
/// Serving before requesting the name is what makes `Type=dbus` units and bus
/// activation safe: the object answers the very first call.
///
/// # Errors
///
/// Returns a [`zbus::Error`] when the object path is already served on
/// `connection`.
pub async fn serve(
    connection: &zbus::Connection,
    publisher: Arc<Publisher>,
    backend: Arc<dyn Backend>,
) -> zbus::Result<()> {
    connection
        .object_server()
        .at(OBJECT_PATH, TokenStation1::new(publisher, backend))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_input_and_a_failed_write_are_different_errors() {
        use crate::backend::SetSettingsError;

        let rejected = as_dbus_error(SetSettingsError::Rejected(vec![
            "general.limits_interval_secs must be 120..=86400".into(),
            "alerts thresholds must be within 1..=100".into(),
        ]));
        assert!(matches!(rejected, fdo::Error::InvalidArgs(_)));
        let message = rejected.to_string();
        assert!(message.contains("limits_interval_secs"), "{message}");
        assert!(message.contains("alerts thresholds"), "{message}");

        // A full disk is not the caller's mistake, and a front end that reports
        // it as one sends the user hunting through their own settings.
        let failed = as_dbus_error(SetSettingsError::Failed(
            "cannot write /home/u/.config/token-station/config.toml: No space left".into(),
        ));
        assert!(matches!(failed, fdo::Error::Failed(_)));
        assert!(failed.to_string().contains("No space left"));
    }

    #[test]
    fn an_oversized_settings_document_still_reads_as_bad_input() {
        let error = as_dbus_error(crate::backend::SetSettingsError::Rejected(vec![format!(
            "settings JSON is larger than {} bytes",
            crate::api::MAX_SETTINGS_BYTES
        )]));
        assert_eq!(
            error.to_string(),
            "org.freedesktop.DBus.Error.InvalidArgs: settings JSON is larger than 262144 bytes"
        );
    }
}
