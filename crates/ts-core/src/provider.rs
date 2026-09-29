//! The interface every usage provider implements.

use std::time::Duration;

use async_trait::async_trait;

use crate::snapshot::{ProviderId, ProviderSnapshot};

/// Result of one plan-limit refresh attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// New data was stored.
    Updated,
    /// Nothing was fetched (backoff, disabled, too soon, token expired...).
    Skipped(String),
    /// The attempt failed; the provider keeps its last good data.
    Failed(String),
}

/// Data pushed into a provider from outside its own polling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ingest {
    /// Raw statusline JSON that Claude Code piped to `token-station statusline`.
    ClaudeStatusline(String),
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IngestError {
    #[error("this provider does not accept that payload")]
    Unsupported,
    #[error("invalid payload: {0}")]
    Invalid(String),
}

/// A source of plan limits and token usage for one CLI.
///
/// Implementations own their state behind interior mutability and must be cheap to
/// call concurrently. They never refresh or write the CLI's credentials.
#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;

    /// Fetch plan limits (network or CLI RPC). `force` bypasses the provider's own
    /// soft interval but never its hard rate-limit backoff.
    async fn refresh_limits(&self, force: bool) -> RefreshOutcome;

    /// Incrementally scan local logs for new token events.
    async fn refresh_tokens(&self);

    /// Current state. Must not block or do IO.
    fn snapshot(&self, now: i64) -> ProviderSnapshot;

    /// Delay before the next `refresh_limits` the provider wants, given `now`
    /// (e.g. longer during 429 backoff, shorter right after a window resets).
    fn next_limits_refresh(&self, now: i64, default_interval: Duration) -> Duration {
        let _ = now;
        default_interval
    }

    /// Accept externally pushed data.
    ///
    /// # Errors
    ///
    /// Returns [`IngestError::Unsupported`] when the provider accepts no pushed
    /// data, which is the default, and an implementation-specific variant when
    /// the payload itself is rejected.
    fn ingest(&self, payload: Ingest, now: i64) -> Result<(), IngestError> {
        let _ = (payload, now);
        Err(IngestError::Unsupported)
    }
}
