//! Fixture mode: serve a recorded snapshot with live countdowns.
//!
//! Front-end developers run `token-station daemon --fixture data/fixtures/snapshot-ok.json`
//! and get the full D-Bus surface without credentials, network or CLIs. Nothing here
//! touches the real config file or the history database.

use std::path::Path;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use ts_core::assemble::assemble;
use ts_core::config::Config;
use ts_core::{ProviderId, Snapshot};

use crate::backend::Backend;
use crate::clock::Clock;
use crate::publish::Publisher;
use crate::settings;

/// Points in a synthetic history series.
pub const SYNTHETIC_POINTS: usize = 60;

#[derive(Debug, thiserror::Error)]
pub enum FixtureError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("{path} is not a valid snapshot: {source}")]
    Parse {
        path: String,
        source: serde_json::Error,
    },
}

/// Serves one recorded snapshot, shifted so its countdowns stay live.
pub struct FixturePlayer {
    base: Snapshot,
    config: RwLock<Config>,
    publisher: Arc<Publisher>,
    clock: Clock,
}

impl FixturePlayer {
    /// Read a snapshot fixture from disk.
    pub fn read_snapshot(path: &Path) -> Result<Snapshot, FixtureError> {
        let text = std::fs::read_to_string(path).map_err(|source| FixtureError::Read {
            path: path.display().to_string(),
            source,
        })?;
        serde_json::from_str(&text).map_err(|source| FixtureError::Parse {
            path: path.display().to_string(),
            source,
        })
    }

    pub fn load(
        path: &Path,
        config: Config,
        clock: Clock,
    ) -> Result<Arc<FixturePlayer>, FixtureError> {
        Ok(FixturePlayer::new(
            FixturePlayer::read_snapshot(path)?,
            config,
            clock,
        ))
    }

    pub fn new(base: Snapshot, config: Config, clock: Clock) -> Arc<FixturePlayer> {
        let first = shift(&base, &config, 1, clock());
        Arc::new(FixturePlayer {
            base,
            config: RwLock::new(config),
            publisher: Publisher::new(first),
            clock,
        })
    }

    pub fn publisher(&self) -> Arc<Publisher> {
        Arc::clone(&self.publisher)
    }

    fn config(&self) -> Config {
        self.config
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The fixture as of now: every timestamp moved, levels re-derived.
    pub fn current(&self) -> Snapshot {
        shift(
            &self.base,
            &self.config(),
            self.publisher.revision(),
            (self.clock)(),
        )
    }

    /// Publish the fixture as of now.
    pub async fn republish(&self) -> bool {
        self.publisher.publish(self.current()).await
    }

    /// Does the fixture contain this window?
    fn knows(&self, provider: ProviderId, window_id: &str) -> Option<f64> {
        self.base
            .providers
            .iter()
            .find(|p| p.id == provider)?
            .windows
            .iter()
            .find(|w| w.id == window_id)
            .map(|w| w.used_percent)
    }
}

/// Re-assemble `base` for `now` under `config`.
fn shift(base: &Snapshot, config: &Config, revision: u64, now: i64) -> Snapshot {
    let shifted = base.shifted(now - base.generated_at);
    assemble(revision, now, &shifted.providers, config)
}

/// A reproducible curve that ends near `end_percent`.
///
/// Fixture history must look plausible and never change between runs, so front ends
/// can screenshot it: the shape depends only on the window id and the time range.
pub fn synthetic_series(
    window_id: &str,
    end_percent: f64,
    since: i64,
    now: i64,
    points: usize,
) -> Vec<(i64, f64)> {
    if points == 0 || now <= since {
        return Vec::new();
    }
    let seed = f64::from(window_id.bytes().map(u32::from).sum::<u32>() % 23);
    let span = (now - since) as f64;
    (0..points)
        .map(|i| {
            let progress = (i + 1) as f64 / points as f64;
            let ts = since + (span * progress) as i64;
            let wobble = 5.0 * (progress * 9.0 + seed).sin();
            let percent = (end_percent * progress + wobble).clamp(0.0, 100.0);
            (ts, (percent * 100.0).round() / 100.0)
        })
        .collect()
}

#[async_trait]
impl Backend for FixturePlayer {
    async fn refresh(&self) {
        self.republish().await;
    }

    async fn history(&self, provider: ProviderId, window_id: &str, since: i64) -> Vec<(i64, f64)> {
        match self.knows(provider, window_id) {
            Some(end_percent) => {
                let now = (self.clock)();
                let since = since.min(now).max(now - 30 * 24 * 60 * 60);
                synthetic_series(window_id, end_percent, since, now, SYNTHETIC_POINTS)
            }
            None => Vec::new(),
        }
    }

    fn settings_json(&self) -> String {
        self.config().to_json()
    }

    /// Applied in memory only: fixture mode never writes the user's config.
    async fn set_settings(&self, json: &str) -> Result<(), Vec<String>> {
        let next = settings::parse_settings(json)?;
        *self.config.write().unwrap_or_else(|e| e.into_inner()) = next;
        self.republish().await;
        Ok(())
    }

    /// Accepted and discarded; there is no provider to feed.
    async fn ingest_claude_statusline(&self, payload: &str) -> Result<(), String> {
        serde_json::from_str::<serde_json::Value>(payload)
            .map(|_| ())
            .map_err(|e| format!("not valid JSON: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::fixed_clock;

    fn fixture_path(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../data/fixtures")
            .join(name)
    }

    fn player(now: i64) -> Arc<FixturePlayer> {
        FixturePlayer::load(
            &fixture_path("snapshot-ok.json"),
            Config::default(),
            fixed_clock(now),
        )
        .unwrap()
    }

    #[test]
    fn every_shipped_fixture_loads() {
        for name in [
            "snapshot-ok.json",
            "snapshot-loading.json",
            "snapshot-near-limit.json",
            "snapshot-not-installed.json",
            "snapshot-stale-auth.json",
        ] {
            FixturePlayer::read_snapshot(&fixture_path(name))
                .unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }

    #[test]
    fn missing_and_broken_fixtures_report_their_path() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.json");
        assert!(
            FixturePlayer::read_snapshot(&missing)
                .unwrap_err()
                .to_string()
                .contains("nope.json")
        );
        let broken = dir.path().join("broken.json");
        std::fs::write(&broken, "{}").unwrap();
        assert!(
            FixturePlayer::read_snapshot(&broken)
                .unwrap_err()
                .to_string()
                .contains("broken.json")
        );
    }

    #[test]
    fn countdowns_follow_the_clock() {
        let now = 1_900_000_000;
        let snapshot = player(now).current();
        assert_eq!(snapshot.generated_at, now);
        let window = &snapshot.providers[0].windows[0];
        // The `snapshot-ok` session window resets 4 h 40 min after it was generated.
        assert_eq!(window.resets_at, Some(now + 16_800));
        assert!(window.observed_at <= now);
    }

    #[test]
    fn levels_come_from_the_current_settings() {
        let now = 1_900_000_000;
        let player = player(now);
        let mut strict = Config::default();
        strict.alerts.warning_percent = 10.0;
        strict.alerts.critical_percent = 20.0;
        *player.config.write().unwrap() = strict;
        assert_eq!(
            player.current().providers[0].windows[0].level,
            ts_core::Level::Critical
        );
    }

    #[tokio::test]
    async fn refresh_reshifts_and_publishes() {
        let player = player(1_900_000_000);
        let first = player.publisher().revision();
        player.refresh().await;
        assert!(player.publisher().revision() >= first);
        let json = player.publisher().snapshot_json();
        assert!(json.contains("\"schemaVersion\":1"));
    }

    #[tokio::test]
    async fn settings_are_applied_in_memory_only() {
        let player = player(1_900_000_000);
        player
            .set_settings(r#"{"alerts":{"warning_percent":42.0}}"#)
            .await
            .unwrap();
        let json = player.settings_json();
        assert!(json.contains("\"warning_percent\":42.0"), "{json}");
        let problems = player
            .set_settings(r#"{"general":{"tokens_interval_secs":1}}"#)
            .await
            .unwrap_err();
        assert_eq!(problems.len(), 1);
    }

    #[tokio::test]
    async fn statusline_is_accepted_and_ignored() {
        let player = player(1_900_000_000);
        player.ingest_claude_statusline("{\"a\":1}").await.unwrap();
        assert!(player.ingest_claude_statusline("nope").await.is_err());
    }

    #[tokio::test]
    async fn history_is_synthetic_and_only_for_known_windows() {
        let now = 1_900_000_000;
        let player = player(now);
        let series = player
            .history(ProviderId::Claude, "session", now - 3_600)
            .await;
        assert_eq!(series.len(), SYNTHETIC_POINTS);
        assert!(series.iter().all(|(_, p)| (0.0..=100.0).contains(p)));
        assert!(series.windows(2).all(|w| w[0].0 <= w[1].0));
        assert_eq!(
            series,
            player
                .history(ProviderId::Claude, "session", now - 3_600)
                .await,
            "must be reproducible"
        );
        assert!(
            player
                .history(ProviderId::Claude, "made-up", now - 60)
                .await
                .is_empty()
        );
        assert!(
            player
                .history(ProviderId::Codex, "session", now - 60)
                .await
                .is_empty()
        );
    }

    #[test]
    fn synthetic_series_handles_degenerate_ranges() {
        assert!(synthetic_series("session", 10.0, 100, 100, 10).is_empty());
        assert!(synthetic_series("session", 10.0, 100, 200, 0).is_empty());
    }
}
