//! What the D-Bus service needs from whoever produces the data.
//!
//! The real daemon and the fixture player both implement this, so the service code
//! (and its tests) never care which one is behind it.

use async_trait::async_trait;
use ts_core::ProviderId;

/// The data source behind `dev.soldunov.TokenStation1`.
#[async_trait]
pub trait Backend: Send + Sync {
    /// Refresh everything now and republish. Returns when the refresh finished.
    async fn refresh(&self);

    /// Recorded samples for one window, oldest first, already down-sampled.
    async fn history(&self, provider: ProviderId, window_id: &str, since: i64) -> Vec<(i64, f64)>;

    /// Current settings as JSON.
    fn settings_json(&self) -> String;

    /// Replace the settings; `Err` lists every problem.
    async fn set_settings(&self, json: &str) -> Result<(), Vec<String>>;

    /// Accept raw Claude Code statusline JSON.
    async fn ingest_claude_statusline(&self, payload: &str) -> Result<(), String>;
}
