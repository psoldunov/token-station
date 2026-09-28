//! Claude Code usage provider (stub; replaced by the real implementation).

use async_trait::async_trait;
use ts_core::config::ClaudeConfig;
use ts_core::pricing::SharedPricing;
use ts_core::{Provider, ProviderId, ProviderSnapshot, ProviderState, RefreshOutcome};

pub struct ClaudeProvider {
    _config: ClaudeConfig,
    _pricing: SharedPricing,
}

impl ClaudeProvider {
    /// Provider using the real process environment (`HOME`, `CLAUDE_CONFIG_DIR`, `PATH`).
    pub fn new(config: ClaudeConfig, pricing: SharedPricing) -> Self {
        ClaudeProvider {
            _config: config,
            _pricing: pricing,
        }
    }
}

#[async_trait]
impl Provider for ClaudeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Claude
    }

    async fn refresh_limits(&self, _force: bool) -> RefreshOutcome {
        RefreshOutcome::Skipped("not implemented".into())
    }

    async fn refresh_tokens(&self) {}

    fn snapshot(&self, _now: i64) -> ProviderSnapshot {
        ProviderSnapshot::empty(ProviderId::Claude, ProviderState::Loading)
    }
}
