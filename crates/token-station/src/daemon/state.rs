//! The live daemon: providers, config, history, alerts and the published snapshot.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use ts_core::alerts::{self, AlertState};
use ts_core::assemble::assemble;
use ts_core::config::Config;
use ts_core::pricing::SharedPricing;
use ts_core::{Ingest, IngestError, Provider, ProviderId, ProviderSnapshot, ProviderState};

use crate::atomic::{modified_secs, write_atomic};
use crate::backend::Backend;
use crate::clock::Clock;
use crate::daemon::providers;
use crate::history::{HistoryHandle, OwnedSample};
use crate::notify::{self, Notifier};
use crate::paths::Paths;
use crate::publish::Publisher;
use crate::settings;

/// Everything one running daemon owns.
pub struct Daemon {
    pub paths: Paths,
    pub publisher: Arc<Publisher>,
    pricing: SharedPricing,
    config: RwLock<Config>,
    config_mtime: Mutex<Option<i64>>,
    providers: RwLock<BTreeMap<ProviderId, Arc<dyn Provider>>>,
    history: HistoryHandle,
    alert_state: Mutex<AlertState>,
    notifier: Arc<dyn Notifier>,
    statusline_seen: Mutex<Option<i64>>,
    last_prune: Mutex<Option<i64>>,
    clock: Clock,
}

impl Daemon {
    /// Assemble a daemon, building its providers from `config`.
    pub fn new(
        paths: Paths,
        config: Config,
        pricing: SharedPricing,
        history: HistoryHandle,
        notifier: Arc<dyn Notifier>,
        clock: Clock,
    ) -> Arc<Daemon> {
        let providers = providers::build_all(&config, &pricing);
        Daemon::with_providers(paths, config, pricing, providers, history, notifier, clock)
    }

    /// Same, with the providers supplied: the seam integration tests use.
    pub fn with_providers(
        paths: Paths,
        config: Config,
        pricing: SharedPricing,
        providers: BTreeMap<ProviderId, Arc<dyn Provider>>,
        history: HistoryHandle,
        notifier: Arc<dyn Notifier>,
        clock: Clock,
    ) -> Arc<Daemon> {
        let now = clock();
        let snapshots = collect(&providers, now);
        let publisher = Publisher::new(assemble(1, now, &snapshots, &config));
        let alert_state = load_alert_state(&paths.alerts_file());
        Arc::new(Daemon {
            config_mtime: Mutex::new(modified_secs(&paths.config_file)),
            paths,
            publisher,
            pricing,
            config: RwLock::new(config),
            providers: RwLock::new(providers),
            history,
            alert_state: Mutex::new(alert_state),
            notifier,
            statusline_seen: Mutex::new(None),
            last_prune: Mutex::new(None),
            clock,
        })
    }

    pub fn now(&self) -> i64 {
        (self.clock)()
    }

    pub fn config(&self) -> Config {
        read(&self.config).clone()
    }

    /// The price table shared with every provider.
    pub fn pricing(&self) -> SharedPricing {
        Arc::clone(&self.pricing)
    }

    pub fn provider(&self, id: ProviderId) -> Option<Arc<dyn Provider>> {
        read(&self.providers).get(&id).cloned()
    }

    fn snapshots(&self) -> Vec<ProviderSnapshot> {
        collect(&read(&self.providers), self.now())
    }

    /// Re-assemble and publish; returns whether front ends were told.
    pub async fn republish(&self) -> bool {
        let now = self.now();
        let snapshot = assemble(
            self.publisher.revision(),
            now,
            &self.snapshots(),
            &read(&self.config),
        );
        self.publisher.publish(snapshot).await
    }

    /// Refresh one provider's plan limits, then record, alert and publish.
    pub async fn refresh_limits(&self, id: ProviderId, force: bool) {
        let Some(provider) = self.provider(id) else {
            return;
        };
        let outcome = provider.refresh_limits(force).await;
        tracing::debug!(provider = %id, ?outcome, "limits refresh");
        self.after_limits().await;
    }

    /// Scan local logs for every provider, then publish.
    pub async fn refresh_tokens(&self) {
        let providers: Vec<Arc<dyn Provider>> = read(&self.providers).values().cloned().collect();
        for provider in providers {
            provider.refresh_tokens().await;
        }
        self.republish().await;
    }

    /// A full forced refresh of everything (`Refresh()`, resume from sleep, startup).
    pub async fn refresh_all(&self, force: bool) {
        let providers: Vec<Arc<dyn Provider>> = read(&self.providers).values().cloned().collect();
        for provider in &providers {
            let outcome = provider.refresh_limits(force).await;
            tracing::debug!(provider = %provider.id(), ?outcome, "limits refresh");
            provider.refresh_tokens().await;
        }
        self.after_limits().await;
    }

    /// Record history, raise notifications and publish.
    async fn after_limits(&self) {
        let now = self.now();
        let snapshots = self.snapshots();
        self.record_history(&snapshots, now).await;
        self.run_alerts(&snapshots, now);
        self.republish().await;
    }

    async fn record_history(&self, snapshots: &[ProviderSnapshot], now: i64) {
        let samples: Vec<OwnedSample> = snapshots
            .iter()
            .filter(|p| matches!(p.state, ProviderState::Ok | ProviderState::Stale))
            .flat_map(|p| {
                p.windows.iter().map(move |w| OwnedSample {
                    provider: p.id,
                    window_id: w.id.clone(),
                    ts: now,
                    percent: w.used_percent,
                })
            })
            .collect();
        if !samples.is_empty() {
            self.history.record(samples).await;
        }
    }

    fn run_alerts(&self, snapshots: &[ProviderSnapshot], now: i64) {
        let config = self.config();
        let mut state = lock(&self.alert_state);
        let (raised, next) = alerts::evaluate(&state, snapshots, &config.alerts);
        if next != *state {
            *state = next;
            if let Err(error) = serde_json::to_vec(&*state)
                .map_err(|e| e.to_string())
                .and_then(|bytes| {
                    write_atomic(&self.paths.alerts_file(), &bytes).map_err(|e| e.to_string())
                })
            {
                tracing::warn!(%error, "cannot persist alert state");
            }
        }
        drop(state);
        notify::deliver(self.notifier.as_ref(), &raised, now);
    }

    /// Drop samples older than the retention window, at most once a day.
    pub async fn prune_history(&self) {
        let config = self.config();
        let now = self.now();
        let due = {
            let mut last = lock(&self.last_prune);
            let due = last.is_none_or(|t| now - t >= 24 * 60 * 60);
            if due {
                *last = Some(now);
            }
            due
        };
        if due {
            let cutoff = now - i64::from(config.general.history_retention_days) * 24 * 60 * 60;
            self.history.prune(cutoff).await;
        }
    }

    /// Re-read `config.toml` when it changed on disk.
    pub async fn reload_config_if_changed(&self) {
        let current = modified_secs(&self.paths.config_file);
        {
            let mut seen = lock(&self.config_mtime);
            if *seen == current {
                return;
            }
            *seen = current;
        }
        let next = settings::reload(&self.paths.config_file, &self.config());
        tracing::info!("config file changed, applying it");
        self.apply_config(next, false).await;
    }

    /// Adopt `next`, rebuilding only what its differences require.
    pub async fn apply_config(&self, next: Config, persist: bool) {
        let change = settings::diff(&self.config(), &next);
        if change.is_empty() {
            return;
        }
        if persist {
            if let Err(error) = settings::persist(&next, &self.paths.config_file) {
                tracing::error!(%error, "cannot write config.toml");
            }
            *lock(&self.config_mtime) = modified_secs(&self.paths.config_file);
        }
        self.rebuild_providers(&next, change);
        *write(&self.config) = next;
        if change.needs_republish() {
            self.republish().await;
        }
    }

    fn rebuild_providers(&self, next: &Config, change: settings::ConfigChange) {
        let rebuild: Vec<ProviderId> = [
            (ProviderId::Claude, change.claude),
            (ProviderId::Codex, change.codex),
        ]
        .into_iter()
        .filter_map(|(id, changed)| changed.then_some(id))
        .collect();
        if rebuild.is_empty() {
            return;
        }
        let mut providers = write(&self.providers);
        for id in rebuild {
            tracing::info!(provider = %id, "rebuilding provider after a settings change");
            providers.insert(id, providers::build_one(id, next, &self.pricing));
        }
    }

    /// Forward a statusline payload to the Claude provider.
    pub fn ingest_statusline(&self, payload: &str) -> Result<(), String> {
        let Some(provider) = self.provider(ProviderId::Claude) else {
            return Err("the Claude provider is not available".into());
        };
        match provider.ingest(Ingest::ClaudeStatusline(payload.to_string()), self.now()) {
            Ok(()) => Ok(()),
            Err(IngestError::Invalid(why)) => Err(why),
            Err(IngestError::Unsupported) => {
                tracing::debug!("provider does not accept statusline payloads");
                Ok(())
            }
        }
    }

    /// Pick up a statusline the CLI could not deliver over the bus.
    pub async fn ingest_statusline_file(&self) {
        let path = self.paths.statusline_drop();
        let Some(modified) = modified_secs(&path) else {
            return;
        };
        {
            let mut seen = lock(&self.statusline_seen);
            if seen.is_some_and(|t| t >= modified) {
                return;
            }
            *seen = Some(modified);
        }
        match std::fs::read_to_string(&path) {
            Ok(payload) => match self.ingest_statusline(&payload) {
                Ok(()) => {
                    self.republish().await;
                }
                Err(error) => tracing::warn!(%error, "dropped statusline file was rejected"),
            },
            Err(error) => tracing::warn!(%error, "cannot read the dropped statusline file"),
        }
    }
}

#[async_trait]
impl Backend for Daemon {
    async fn refresh(&self) {
        self.refresh_all(true).await;
    }

    async fn history(&self, provider: ProviderId, window_id: &str, since: i64) -> Vec<(i64, f64)> {
        self.history
            .query(provider, window_id.to_string(), since)
            .await
    }

    fn settings_json(&self) -> String {
        self.config().to_json()
    }

    async fn set_settings(&self, json: &str) -> Result<(), Vec<String>> {
        let next = settings::parse_settings(json)?;
        self.apply_config(next, true).await;
        Ok(())
    }

    async fn ingest_claude_statusline(&self, payload: &str) -> Result<(), String> {
        self.ingest_statusline(payload)?;
        self.republish().await;
        Ok(())
    }
}

fn collect(providers: &BTreeMap<ProviderId, Arc<dyn Provider>>, now: i64) -> Vec<ProviderSnapshot> {
    providers.values().map(|p| p.snapshot(now)).collect()
}

fn load_alert_state(path: &std::path::Path) -> AlertState {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
            tracing::warn!(%error, "ignoring unreadable alert state");
            AlertState::default()
        }),
        Err(_) => AlertState::default(),
    }
}

fn read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|e| e.into_inner())
}

fn write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(|e| e.into_inner())
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
