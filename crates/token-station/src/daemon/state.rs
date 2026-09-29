//! The live daemon: providers, config, history, alerts and the published snapshot.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock, Weak};

use async_trait::async_trait;
use ts_core::alerts::{self, Alert, AlertState};
use ts_core::assemble::assemble;
use ts_core::config::Config;
use ts_core::pricing::SharedPricing;
use ts_core::{Ingest, IngestError, Provider, ProviderId, ProviderSnapshot, ProviderState};

use crate::atomic::{Fingerprint, fingerprint, modified_secs, write_atomic};
use crate::backend::{Backend, SetSettingsError};
use crate::clock::Clock;
use crate::daemon::providers;
use crate::history::{HistoryHandle, OwnedSample};
use crate::notify::{self, Notifier};
use crate::paths::Paths;
use crate::pricing_refresh;
use crate::publish::Publisher;
use crate::settings;

/// A dropped statusline older than this describes a session that is long over.
pub const STATUSLINE_DROP_MAX_AGE_SECS: i64 = 10 * 60;

/// Everything one running daemon owns.
pub struct Daemon {
    /// The `Arc` this daemon lives in, so work that outlives one method call can
    /// be spawned. Weak, or the daemon would keep itself alive for ever.
    me: Weak<Daemon>,
    pub paths: Paths,
    pub publisher: Arc<Publisher>,
    pricing: SharedPricing,
    config: RwLock<Config>,
    /// Identity of `config.toml` as last seen, for the hot reload.
    config_stamp: Mutex<Option<Fingerprint>>,
    /// Held for the whole of [`Daemon::apply_config`], so two settings changes
    /// cannot interleave their rebuilds.
    apply_lock: tokio::sync::Mutex<()>,
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
    ///
    /// Blocking: building a provider reads the disk (`~/.claude/.credentials.json`,
    /// `PATH`), so the daemon is only ever built on a blocking thread.
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
        Arc::new_cyclic(|me| Daemon {
            me: me.clone(),
            config_stamp: Mutex::new(fingerprint(&paths.config_file)),
            apply_lock: tokio::sync::Mutex::new(()),
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
        self.run_alerts(&snapshots, now).await;
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

    async fn run_alerts(&self, snapshots: &[ProviderSnapshot], now: i64) {
        let (raised, to_store) = self.advance_alert_state(snapshots);
        if let Some(bytes) = to_store {
            self.store_alert_state(bytes).await;
        }
        notify::deliver(self.notifier.as_ref(), &raised, now).await;
    }

    /// Fold `snapshots` into the alert state: what to announce, and the JSON to
    /// store when the state actually moved.
    fn advance_alert_state(&self, snapshots: &[ProviderSnapshot]) -> (Vec<Alert>, Option<Vec<u8>>) {
        let config = self.config();
        let mut state = lock(&self.alert_state);
        let (raised, next) = alerts::evaluate(&state, snapshots, &config.alerts);
        if next == *state {
            return (raised, None);
        }
        *state = next;
        match serde_json::to_vec(&*state) {
            Ok(bytes) => (raised, Some(bytes)),
            Err(error) => {
                tracing::warn!(%error, "cannot serialise alert state");
                (raised, None)
            }
        }
    }

    /// `write_atomic` fsyncs, so the write belongs on a blocking thread.
    async fn store_alert_state(&self, bytes: Vec<u8>) {
        let path = self.paths.alerts_file();
        match tokio::task::spawn_blocking(move || write_atomic(&path, &bytes)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!(%error, "cannot persist alert state"),
            Err(error) => tracing::warn!(%error, "the alert-state write did not run"),
        }
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
        let current = fingerprint(&self.paths.config_file);
        {
            let mut seen = lock(&self.config_stamp);
            if *seen == current {
                return;
            }
            *seen = current;
        }
        let next = settings::reload(&self.paths.config_file, &self.config());
        tracing::info!("config file changed, applying it");
        if let Err(error) = self.apply_config(next, false).await {
            tracing::warn!(%error, "cannot apply the reloaded config");
        }
    }

    /// Adopt `next`, rebuilding only what its differences require.
    ///
    /// Two steps. Everything a caller has to see before its call returns — the
    /// write, the swap, the rebuild, the republish — happens under `apply_lock`, so
    /// two `SetSettings` calls cannot interleave a rebuild with a store and leave
    /// the providers built from a config nobody kept. Reading the rebuilt providers
    /// and refreshing prices then happens on its own task, after the lock is
    /// released: both talk to the network, and a `SetSettings` reply that waits for
    /// them arrives after the caller's own D-Bus timeout has expired.
    pub async fn apply_config(&self, next: Config, persist: bool) -> Result<(), SetSettingsError> {
        let (rebuilt, pricing) = {
            let _serialised = self.apply_lock.lock().await;
            let change = settings::diff(&self.config(), &next);
            if change.is_empty() {
                return Ok(());
            }
            if persist {
                self.persist_config(&next).await?;
            }
            let rebuilt = self.rebuild_providers(&next, change).await;
            *write(&self.config) = next;
            if change.needs_republish() {
                self.republish().await;
            }
            (rebuilt, change.pricing)
        };
        self.spawn_fill_in(rebuilt, pricing);
        Ok(())
    }

    /// Write `next` to `config.toml`, off the runtime's threads: the write fsyncs.
    async fn persist_config(&self, next: &Config) -> Result<(), SetSettingsError> {
        let path = self.paths.config_file.clone();
        let config = next.clone();
        let written = tokio::task::spawn_blocking(move || {
            settings::persist(&config, &path).map(|()| fingerprint(&path))
        })
        .await;
        match written {
            Ok(Ok(stamp)) => {
                *lock(&self.config_stamp) = stamp;
                Ok(())
            }
            Ok(Err(error)) => Err(error.into()),
            Err(error) => Err(SetSettingsError::Failed(format!(
                "the settings write did not run: {error}"
            ))),
        }
    }

    /// Read the rebuilt providers and refresh prices, once the caller has its reply.
    fn spawn_fill_in(&self, rebuilt: Vec<ProviderId>, pricing: bool) {
        if rebuilt.is_empty() && !pricing {
            return;
        }
        let Some(daemon) = self.me.upgrade() else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::debug!("no runtime: nothing to fill in after the settings change");
            return;
        };
        runtime.spawn(async move {
            daemon.fill_in(&rebuilt).await;
            if pricing {
                daemon.refresh_pricing().await;
            }
        });
    }

    /// A rebuilt provider starts out `Loading`; read it now rather than leaving a
    /// blank tile until the next limits tick, up to [`MAX_SLEEP`] away.
    ///
    /// [`MAX_SLEEP`]: crate::daemon::scheduler::MAX_SLEEP
    async fn fill_in(&self, rebuilt: &[ProviderId]) {
        if rebuilt.is_empty() {
            return;
        }
        for id in rebuilt {
            self.refresh_limits(*id, false).await;
        }
        self.refresh_tokens().await;
    }

    /// Update the price table when auto-update is on and the cache is due.
    pub async fn refresh_pricing(&self) {
        let config = self.config();
        let cache = self.paths.pricing_cache();
        pricing_refresh::refresh_if_due(&config.pricing, &cache, &self.pricing(), self.now()).await;
    }

    /// Replace the providers whose settings changed; returns which ones moved.
    ///
    /// Built on a blocking thread and only then swapped in: constructing a provider
    /// reads credentials and walks `PATH`, which belongs neither on a runtime
    /// thread nor inside the `providers` write lock every `snapshot()` needs.
    async fn rebuild_providers(
        &self,
        next: &Config,
        change: settings::ConfigChange,
    ) -> Vec<ProviderId> {
        let rebuild: Vec<ProviderId> = [
            (ProviderId::Claude, change.claude),
            (ProviderId::Codex, change.codex),
        ]
        .into_iter()
        .filter_map(|(id, changed)| changed.then_some(id))
        .collect();
        if rebuild.is_empty() {
            return rebuild;
        }
        let (config, pricing, ids) = (next.clone(), self.pricing(), rebuild.clone());
        let built = tokio::task::spawn_blocking(move || {
            ids.into_iter()
                .map(|id| {
                    tracing::info!(provider = %id, "rebuilding provider after a settings change");
                    (id, providers::build_one(id, &config, &pricing))
                })
                .collect::<Vec<_>>()
        })
        .await;
        match built {
            Ok(built) => {
                let mut providers = write(&self.providers);
                for (id, provider) in built {
                    providers.insert(id, provider);
                }
                rebuild
            }
            Err(error) => {
                tracing::warn!(%error, "the provider rebuild did not run");
                Vec::new()
            }
        }
    }

    /// Forward a statusline payload to the Claude provider as seen at `observed_at`.
    pub fn ingest_statusline(&self, payload: &str, observed_at: i64) -> Result<(), String> {
        let Some(provider) = self.provider(ProviderId::Claude) else {
            return Err("the Claude provider is not available".into());
        };
        match provider.ingest(Ingest::ClaudeStatusline(payload.to_string()), observed_at) {
            Ok(()) => Ok(()),
            Err(IngestError::Invalid(why)) => Err(why),
            Err(IngestError::Unsupported) => {
                tracing::debug!("provider does not accept statusline payloads");
                Ok(())
            }
        }
    }

    /// Pick up a statusline the CLI could not deliver over the bus.
    ///
    /// The file's mtime is the observation time: after a restart the drop box may
    /// hold days-old numbers, and stamping those with `now()` would let them
    /// override the fresh figures the OAuth reader just produced.
    pub async fn ingest_statusline_file(&self) {
        let path = self.paths.statusline_drop();
        let Some(modified) = modified_secs(&path) else {
            return;
        };
        let age = self.now() - modified;
        if age > STATUSLINE_DROP_MAX_AGE_SECS {
            tracing::debug!(age, "ignoring a dropped statusline from an old session");
            return;
        }
        {
            let mut seen = lock(&self.statusline_seen);
            if seen.is_some_and(|t| t >= modified) {
                return;
            }
            *seen = Some(modified);
        }
        // Off the runtime's threads: the drop box is a file in the runtime
        // directory, but this runs on the tokens tick and a read is a read.
        match tokio::task::spawn_blocking(move || std::fs::read_to_string(&path)).await {
            Ok(Ok(payload)) => match self.ingest_statusline(&payload, modified) {
                Ok(()) => {
                    self.republish().await;
                }
                Err(error) => tracing::warn!(%error, "dropped statusline file was rejected"),
            },
            Ok(Err(error)) => tracing::warn!(%error, "cannot read the dropped statusline file"),
            Err(error) => tracing::warn!(%error, "the statusline file read did not run"),
        }
    }

    /// Fold the downloaded price table on disk over the bundled one.
    ///
    /// Deliberately not part of building the daemon: that happens before the bus
    /// name is claimed, and a daemon that then loses the race should have read
    /// nothing at all.
    pub async fn merge_pricing_cache(&self) {
        let cache = self.paths.pricing_cache();
        match tokio::task::spawn_blocking(move || pricing_refresh::load_cache(&cache)).await {
            Ok(Some(cached)) => pricing_refresh::install(&self.pricing(), &cached),
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "the pricing cache read did not run"),
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

    async fn set_settings(&self, json: &str) -> Result<(), SetSettingsError> {
        let next = settings::parse_settings(json).map_err(SetSettingsError::Rejected)?;
        self.apply_config(next, true).await
    }

    async fn ingest_claude_statusline(&self, payload: &str) -> Result<(), String> {
        self.ingest_statusline(payload, self.now())?;
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
