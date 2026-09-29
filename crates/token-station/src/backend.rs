//! What the D-Bus service needs from whoever produces the data.
//!
//! The real daemon and the fixture player both implement this, so the service code
//! (and its tests) never care which one is behind it.

use async_trait::async_trait;
use ts_core::ProviderId;

use crate::settings::PersistError;

/// Why a `SetSettings` call did not take effect.
///
/// The distinction is what the caller sees on the bus: `Rejected` becomes
/// `org.freedesktop.DBus.Error.InvalidArgs`, which tells a front end the document
/// it sent is the problem, and `Failed` becomes
/// `org.freedesktop.DBus.Error.Failed`, which says the settings were fine and the
/// daemon could not carry them out. Reporting a full disk as `InvalidArgs` sends
/// the user looking for a mistake in their own input.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SetSettingsError {
    /// Something the caller can fix: a validation problem, or a `config.toml`
    /// that is somebody else's to change.
    #[error("{}", .0.join("; "))]
    Rejected(Vec<String>),
    /// The settings were valid but could not be applied.
    #[error("{0}")]
    Failed(String),
}

impl SetSettingsError {
    /// Every problem, as a front end would list them.
    pub fn problems(&self) -> Vec<String> {
        match self {
            SetSettingsError::Rejected(problems) => problems.clone(),
            SetSettingsError::Failed(message) => vec![message.clone()],
        }
    }
}

impl From<PersistError> for SetSettingsError {
    fn from(error: PersistError) -> SetSettingsError {
        match error {
            // Not a bad request, and not the daemon's to fix either: the user has
            // to change it where it is managed, so the input is what is refused.
            PersistError::Managed => SetSettingsError::Rejected(vec![error.to_string()]),
            PersistError::Write(message) => SetSettingsError::Failed(message),
        }
    }
}

/// The data source behind `dev.soldunov.TokenStation1`.
#[async_trait]
pub trait Backend: Send + Sync {
    /// Refresh everything now and republish. Returns when the refresh finished.
    async fn refresh(&self);

    /// Recorded samples for one window, oldest first, already down-sampled.
    async fn history(&self, provider: ProviderId, window_id: &str, since: i64) -> Vec<(i64, f64)>;

    /// Current settings as JSON.
    fn settings_json(&self) -> String;

    /// Replace the settings; `Err` says whether the document or the daemon was
    /// at fault. See [`SetSettingsError`].
    async fn set_settings(&self, json: &str) -> Result<(), SetSettingsError>;

    /// Accept raw Claude Code statusline JSON.
    async fn ingest_claude_statusline(&self, payload: &str) -> Result<(), String>;
}
