//! The service, without a transport.
//!
//! Both front doors — the D-Bus interface on Linux and the JSON-RPC socket on
//! macOS — serve the same methods with the same arguments, the same limits and
//! the same errors; only the framing differs. That contract lives here, so the
//! two adapters cannot drift apart and so it can be tested without a bus or a
//! socket. See `docs/dbus-api.md` and `docs/socket-api.md`.

use std::sync::Arc;

use crate::backend::{Backend, SetSettingsError};
use crate::coalesce::Coalescer;
use crate::history;
use crate::publish::Publisher;

/// Settings documents are small; anything larger is not a config.
pub const MAX_SETTINGS_BYTES: usize = 256 * 1024;
/// Largest accepted `IngestClaudeStatusline` payload.
pub const MAX_STATUSLINE_BYTES: usize = 64 * 1024;
/// Longest accepted window id in `GetHistory`.
pub const MAX_WINDOW_ID_LEN: usize = 128;

/// Accept only a known provider and a plausible window id.
///
/// An unusable pair is not an error: `GetHistory` answers it with an empty
/// series, so a front end asking about a provider this build does not know
/// draws an empty chart rather than showing a fault.
#[must_use]
pub fn validate_history_args(
    provider: &str,
    window_id: &str,
) -> Option<(ts_core::ProviderId, String)> {
    let provider = provider.parse::<ts_core::ProviderId>().ok()?;
    let usable = !window_id.is_empty()
        && window_id.len() <= MAX_WINDOW_ID_LEN
        && !window_id.chars().any(char::is_control);
    usable.then(|| (provider, window_id.to_string()))
}

/// Serves the published snapshot and forwards calls to a [`Backend`].
pub struct Api {
    publisher: Arc<Publisher>,
    backend: Arc<dyn Backend>,
    refresh_gate: Arc<Coalescer>,
}

impl Api {
    #[must_use]
    pub fn new(publisher: Arc<Publisher>, backend: Arc<dyn Backend>) -> Api {
        Api {
            publisher,
            backend,
            refresh_gate: Arc::new(Coalescer::default()),
        }
    }

    /// Full state as JSON.
    #[must_use]
    pub fn snapshot_json(&self) -> String {
        self.publisher.snapshot_json()
    }

    /// Increments with every new snapshot.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.publisher.revision()
    }

    /// The revision and the snapshot JSON that goes with it.
    #[must_use]
    pub fn published(&self) -> (u64, String) {
        self.publisher.published()
    }

    /// Refresh every provider now; concurrent callers share one run.
    pub async fn refresh(&self) {
        let backend = Arc::clone(&self.backend);
        self.refresh_gate.run(|| backend.refresh()).await;
    }

    /// Recorded samples for one window since `since` (Unix seconds), as JSON.
    pub async fn history_json(&self, provider: &str, window_id: &str, since: u64) -> String {
        let Some((provider, window_id)) = validate_history_args(provider, window_id) else {
            return "[]".into();
        };
        let since = i64::try_from(since).unwrap_or(i64::MAX);
        let points = self.backend.history(provider, &window_id, since).await;
        history::to_json(&points)
    }

    /// Current config as JSON.
    #[must_use]
    pub fn settings_json(&self) -> String {
        self.backend.settings_json()
    }

    /// Replace the config.
    ///
    /// # Errors
    ///
    /// Returns [`SetSettingsError::Rejected`] for a document that is too large
    /// or that the backend refused, and [`SetSettingsError::Failed`] when the
    /// settings were fine and the write was not.
    pub async fn set_settings(&self, json: &str) -> Result<(), SetSettingsError> {
        if json.len() > MAX_SETTINGS_BYTES {
            return Err(SetSettingsError::Rejected(vec![format!(
                "settings JSON is larger than {MAX_SETTINGS_BYTES} bytes"
            )]));
        }
        self.backend.set_settings(json).await
    }

    /// Raw statusline JSON piped by `token-station statusline`.
    ///
    /// # Errors
    ///
    /// Returns a message when the payload is larger than
    /// [`MAX_STATUSLINE_BYTES`] or is not a statusline document.
    pub async fn ingest_claude_statusline(&self, json: &str) -> Result<(), String> {
        if json.len() > MAX_STATUSLINE_BYTES {
            return Err(format!(
                "statusline payload is larger than {MAX_STATUSLINE_BYTES} bytes"
            ));
        }
        self.backend.ingest_claude_statusline(json).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts_core::ProviderId;

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

    #[test]
    fn the_limits_are_the_ones_both_transports_document() {
        assert_eq!(MAX_SETTINGS_BYTES, 262_144);
        assert_eq!(MAX_STATUSLINE_BYTES, 65_536);
        assert_eq!(MAX_WINDOW_ID_LEN, 128);
    }
}
