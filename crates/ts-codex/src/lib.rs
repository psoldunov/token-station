//! Codex usage provider (stub; replaced by the real implementation).

use async_trait::async_trait;
use ts_core::config::CodexConfig;
use ts_core::pricing::SharedPricing;
use ts_core::{Provider, ProviderId, ProviderSnapshot, ProviderState, RefreshOutcome};

pub struct CodexProvider {
    _config: CodexConfig,
    _pricing: SharedPricing,
}

impl CodexProvider {
    /// Provider using the real process environment (`HOME`, `CODEX_HOME`, `PATH`).
    pub fn new(config: CodexConfig, pricing: SharedPricing) -> Self {
        CodexProvider {
            _config: config,
            _pricing: pricing,
        }
    }
}

#[async_trait]
impl Provider for CodexProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Codex
    }

    async fn refresh_limits(&self, _force: bool) -> RefreshOutcome {
        RefreshOutcome::Skipped("not implemented".into())
    }

    async fn refresh_tokens(&self) {}

    fn snapshot(&self, _now: i64) -> ProviderSnapshot {
        ProviderSnapshot::empty(ProviderId::Codex, ProviderState::Loading)
    }
}
