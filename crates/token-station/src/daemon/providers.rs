//! Turning config into the set of live providers.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use ts_core::config::Config;
use ts_core::pricing::SharedPricing;
use ts_core::{Provider, ProviderId, ProviderSnapshot, ProviderState, RefreshOutcome};

/// Stands in for a provider the user turned off.
pub struct DisabledProvider {
    id: ProviderId,
}

impl DisabledProvider {
    pub fn new(id: ProviderId) -> DisabledProvider {
        DisabledProvider { id }
    }
}

#[async_trait]
impl Provider for DisabledProvider {
    fn id(&self) -> ProviderId {
        self.id
    }

    async fn refresh_limits(&self, _force: bool) -> RefreshOutcome {
        RefreshOutcome::Skipped("disabled".into())
    }

    async fn refresh_tokens(&self) {}

    fn snapshot(&self, _now: i64) -> ProviderSnapshot {
        ProviderSnapshot {
            message: Some("Turned off in settings".into()),
            ..ProviderSnapshot::empty(self.id, ProviderState::Disabled)
        }
    }
}

/// The provider for `id` as `config` describes it.
pub fn build_one(id: ProviderId, config: &Config, pricing: &SharedPricing) -> Arc<dyn Provider> {
    match id {
        ProviderId::Claude if config.claude.enabled => Arc::new(ts_claude::ClaudeProvider::new(
            config.claude.clone(),
            pricing.clone(),
        )),
        ProviderId::Codex if config.codex.enabled => Arc::new(ts_codex::CodexProvider::new(
            config.codex.clone(),
            pricing.clone(),
        )),
        other => Arc::new(DisabledProvider::new(other)),
    }
}

/// Every provider, in display order.
pub fn build_all(
    config: &Config,
    pricing: &SharedPricing,
) -> BTreeMap<ProviderId, Arc<dyn Provider>> {
    ProviderId::ALL
        .iter()
        .map(|id| (*id, build_one(*id, config, pricing)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts_core::pricing::shared_bundled;

    #[tokio::test]
    async fn disabled_providers_report_themselves_as_off() {
        let mut config = Config::default();
        config.claude.enabled = false;
        config.codex.enabled = false;
        let built = build_all(&config, &shared_bundled());
        assert_eq!(built.len(), 2);
        for (id, provider) in &built {
            let snapshot = provider.snapshot(0);
            assert_eq!(snapshot.id, *id);
            assert_eq!(snapshot.state, ProviderState::Disabled);
            assert_eq!(snapshot.message.as_deref(), Some("Turned off in settings"));
            assert_eq!(
                provider.refresh_limits(true).await,
                RefreshOutcome::Skipped("disabled".into())
            );
            provider.refresh_tokens().await;
        }
    }

    #[test]
    fn enabled_providers_are_the_real_ones() {
        let built = build_all(&Config::default(), &shared_bundled());
        assert_eq!(built[&ProviderId::Claude].id(), ProviderId::Claude);
        assert_ne!(
            built[&ProviderId::Claude].snapshot(0).state,
            ProviderState::Disabled
        );
        assert_ne!(
            built[&ProviderId::Codex].snapshot(0).state,
            ProviderState::Disabled
        );
    }
}
