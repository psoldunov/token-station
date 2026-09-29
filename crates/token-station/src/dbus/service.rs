//! The `dev.soldunov.TokenStation1` interface implementation.

use std::sync::Arc;

use zbus::fdo;

use crate::backend::Backend;
use crate::coalesce::Coalescer;
use crate::dbus::{MAX_STATUSLINE_BYTES, MAX_WINDOW_ID_LEN, OBJECT_PATH};
use crate::history;
use crate::publish::Publisher;

/// Serves the published snapshot and forwards calls to a [`Backend`].
pub struct TokenStation1 {
    publisher: Arc<Publisher>,
    backend: Arc<dyn Backend>,
    refresh_gate: Arc<Coalescer>,
}

impl TokenStation1 {
    pub fn new(publisher: Arc<Publisher>, backend: Arc<dyn Backend>) -> TokenStation1 {
        TokenStation1 {
            publisher,
            backend,
            refresh_gate: Arc::new(Coalescer::default()),
        }
    }
}

#[zbus::interface(name = "dev.soldunov.TokenStation1")]
impl TokenStation1 {
    /// Full state as JSON.
    #[zbus(property)]
    fn snapshot(&self) -> String {
        self.publisher.snapshot_json()
    }

    /// Increments with every new snapshot.
    #[zbus(property)]
    fn revision(&self) -> u64 {
        self.publisher.revision()
    }

    /// Refresh every provider now; concurrent callers share one run.
    async fn refresh(&self) {
        let backend = Arc::clone(&self.backend);
        self.refresh_gate.run(|| backend.refresh()).await;
    }

    /// Recorded samples for one window since `since` (Unix seconds).
    async fn get_history(&self, provider: &str, window_id: &str, since: u64) -> String {
        let Some((provider, window_id)) = validate_history_args(provider, window_id) else {
            return "[]".into();
        };
        let since = i64::try_from(since).unwrap_or(i64::MAX);
        let points = self.backend.history(provider, &window_id, since).await;
        history::to_json(&points)
    }

    /// Current config as JSON.
    fn get_settings(&self) -> String {
        self.backend.settings_json()
    }

    /// Replace the config; invalid input lists every problem.
    async fn set_settings(&self, json: &str) -> fdo::Result<()> {
        if json.len() > MAX_SETTINGS_BYTES {
            return Err(fdo::Error::InvalidArgs(format!(
                "settings JSON is larger than {MAX_SETTINGS_BYTES} bytes"
            )));
        }
        self.backend.set_settings(json).await.map_err(as_dbus_error)
    }

    /// Raw statusline JSON piped by `token-station statusline`.
    async fn ingest_claude_statusline(&self, json: &str) -> fdo::Result<()> {
        if json.len() > MAX_STATUSLINE_BYTES {
            return Err(fdo::Error::InvalidArgs(format!(
                "statusline payload is larger than {MAX_STATUSLINE_BYTES} bytes"
            )));
        }
        self.backend
            .ingest_claude_statusline(json)
            .await
            .map_err(fdo::Error::InvalidArgs)
    }
}

/// Settings documents are small; anything larger is not a config.
const MAX_SETTINGS_BYTES: usize = 256 * 1024;

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

/// Accept only a known provider and a plausible window id.
fn validate_history_args(provider: &str, window_id: &str) -> Option<(ts_core::ProviderId, String)> {
    let provider = provider.parse::<ts_core::ProviderId>().ok()?;
    let usable = !window_id.is_empty()
        && window_id.len() <= MAX_WINDOW_ID_LEN
        && !window_id.chars().any(char::is_control);
    usable.then(|| (provider, window_id.to_string()))
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
    use ts_core::ProviderId;

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
    fn history_arguments_are_validated() {
        assert_eq!(
            validate_history_args("claude", "session"),
            Some((ProviderId::Claude, "session".to_string()))
        );
        assert!(validate_history_args("gemini", "session").is_none());
        assert!(validate_history_args("claude", "").is_none());
        assert!(validate_history_args("claude", "a\u{0}b").is_none());
        assert!(validate_history_args("claude", &"x".repeat(129)).is_none());
        assert!(validate_history_args("claude", &"x".repeat(128)).is_some());
    }
}
