//! The live daemon: refresh, history, alerts, settings and the statusline drop box.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use token_station::backend::Backend;
use token_station::clock::fixed_clock;
use token_station::daemon::Daemon;
use token_station::history::{HistoryHandle, HistoryStore};
use token_station::notify::Notifier;
use token_station::paths::{Env, Paths};
use token_station::settings;
use ts_core::config::Config;
use ts_core::pricing::shared_bundled;
use ts_core::{
    Ingest, IngestError, Level, Provider, ProviderId, ProviderSnapshot, ProviderState,
    RefreshOutcome, UsageWindow, WindowKind,
};

const NOW: i64 = 1_790_596_800;

/// A provider under the test's control.
struct FakeProvider {
    id: ProviderId,
    percent: Mutex<f64>,
    limits_refreshes: AtomicUsize,
    token_refreshes: AtomicUsize,
    ingested: Mutex<Vec<String>>,
    /// The `now` each ingest was handed, so the drop box's mtime can be asserted.
    observed_at: Mutex<Vec<i64>>,
}

impl FakeProvider {
    fn new(id: ProviderId, percent: f64) -> Arc<FakeProvider> {
        Arc::new(FakeProvider {
            id,
            percent: Mutex::new(percent),
            limits_refreshes: AtomicUsize::new(0),
            token_refreshes: AtomicUsize::new(0),
            ingested: Mutex::new(Vec::new()),
            observed_at: Mutex::new(Vec::new()),
        })
    }

    fn set_percent(&self, percent: f64) {
        *self.percent.lock().unwrap() = percent;
    }
}

#[async_trait]
impl Provider for FakeProvider {
    fn id(&self) -> ProviderId {
        self.id
    }

    async fn refresh_limits(&self, _force: bool) -> RefreshOutcome {
        self.limits_refreshes.fetch_add(1, Ordering::SeqCst);
        RefreshOutcome::Updated
    }

    async fn refresh_tokens(&self) {
        self.token_refreshes.fetch_add(1, Ordering::SeqCst);
    }

    fn snapshot(&self, now: i64) -> ProviderSnapshot {
        ProviderSnapshot {
            windows: vec![UsageWindow {
                id: "session".into(),
                label: "Session".into(),
                kind: WindowKind::Session,
                used_percent: *self.percent.lock().unwrap(),
                resets_at: Some(NOW + 8_040),
                window_minutes: Some(300),
                level: Level::Normal,
                source: "fake".into(),
                observed_at: now,
            }],
            updated_at: Some(now),
            ..ProviderSnapshot::empty(self.id, ProviderState::Ok)
        }
    }

    fn ingest(&self, payload: Ingest, now: i64) -> Result<(), IngestError> {
        let Ingest::ClaudeStatusline(text) = payload;
        if text.contains("bad") {
            return Err(IngestError::Invalid("unrecognised statusline".into()));
        }
        self.ingested.lock().unwrap().push(text);
        self.observed_at.lock().unwrap().push(now);
        Ok(())
    }
}

#[derive(Default)]
struct RecordingNotifier {
    sent: Mutex<Vec<String>>,
}

#[async_trait]
impl Notifier for RecordingNotifier {
    async fn notify(&self, summary: &str, _body: &str, _level: Level) {
        self.sent.lock().unwrap().push(summary.to_string());
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    daemon: Arc<Daemon>,
    claude: Arc<FakeProvider>,
    notifier: Arc<RecordingNotifier>,
    paths: Paths,
}

fn harness(percent: f64) -> Harness {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().to_string_lossy().into_owned();
    let paths = Paths::resolve(&Env {
        home: Some(root.clone()),
        config_home: Some(format!("{root}/config")),
        state_home: Some(format!("{root}/state")),
        cache_home: Some(format!("{root}/cache")),
        runtime_dir: Some(format!("{root}/run")),
        data_home: Some(format!("{root}/data")),
    });

    let claude = FakeProvider::new(ProviderId::Claude, percent);
    let codex = FakeProvider::new(ProviderId::Codex, 0.0);
    let providers: BTreeMap<ProviderId, Arc<dyn Provider>> = BTreeMap::from([
        (ProviderId::Claude, Arc::clone(&claude) as Arc<dyn Provider>),
        (ProviderId::Codex, codex as Arc<dyn Provider>),
    ]);

    let notifier = Arc::new(RecordingNotifier::default());
    let history = HistoryHandle::new(Arc::new(HistoryStore::in_memory().expect("sqlite")));
    let daemon = Daemon::with_providers(
        paths.clone(),
        Config::default(),
        shared_bundled(),
        providers,
        history,
        Arc::clone(&notifier) as Arc<dyn Notifier>,
        fixed_clock(NOW),
    );
    Harness {
        _dir: dir,
        daemon,
        claude,
        notifier,
        paths,
    }
}

#[tokio::test]
async fn a_limits_refresh_records_history_publishes_and_notifies() {
    let h = harness(0.0);
    assert_eq!(h.daemon.publisher.revision(), 1);

    h.claude.set_percent(82.0);
    h.daemon.refresh_limits(ProviderId::Claude, false).await;

    assert_eq!(h.claude.limits_refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(h.daemon.publisher.revision(), 2);

    let points = h.daemon.history(ProviderId::Claude, "session", 0).await;
    assert_eq!(points, vec![(NOW, 82.0)]);

    assert_eq!(
        *h.notifier.sent.lock().unwrap(),
        vec!["Claude Code: Session at 82 %".to_string()]
    );
    // The alert state survives a restart.
    let persisted = std::fs::read_to_string(h.paths.alerts_file()).expect("alerts.json written");
    assert!(persisted.contains("claude/session"), "{persisted}");
}

#[tokio::test]
async fn a_threshold_fires_once_per_period() {
    let h = harness(82.0);
    h.daemon.refresh_all(true).await;
    h.claude.set_percent(84.0);
    h.daemon.refresh_all(true).await;
    assert_eq!(h.notifier.sent.lock().unwrap().len(), 1);

    h.claude.set_percent(99.0);
    h.daemon.refresh_all(true).await;
    assert_eq!(h.notifier.sent.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn an_unchanged_snapshot_does_not_bump_the_revision() {
    let h = harness(12.0);
    h.daemon.refresh_all(true).await;
    let revision = h.daemon.publisher.revision();
    h.daemon.refresh_all(true).await;
    assert_eq!(h.daemon.publisher.revision(), revision);
    assert_eq!(h.claude.token_refreshes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn set_settings_writes_the_config_and_applies_it() {
    let h = harness(70.0);
    h.daemon
        .set_settings(r#"{"alerts":{"warning_percent":60.0,"critical_percent":90.0}}"#)
        .await
        .expect("accepted");

    assert_eq!(h.daemon.config().alerts.warning_percent, 60.0);
    let written = std::fs::read_to_string(&h.paths.config_file).expect("config.toml written");
    assert!(written.contains("warning_percent = 60.0"), "{written}");

    // The new threshold is reflected in the published snapshot.
    let json = h.daemon.publisher.snapshot_json();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["providers"][0]["windows"][0]["level"], "warning");
}

#[tokio::test]
async fn invalid_settings_are_rejected_without_touching_the_file() {
    let h = harness(10.0);
    let problems = h
        .daemon
        .set_settings(r#"{"general":{"limits_interval_secs":5}}"#)
        .await
        .unwrap_err();
    assert_eq!(problems.len(), 1);
    assert!(!h.paths.config_file.exists());
    assert_eq!(h.daemon.config(), Config::default());
}

#[tokio::test]
async fn the_config_file_is_hot_reloaded() {
    let h = harness(10.0);
    std::fs::create_dir_all(h.paths.config_file.parent().unwrap()).unwrap();
    std::fs::write(&h.paths.config_file, "[alerts]\nwarning_percent = 55.0\n").unwrap();

    h.daemon.reload_config_if_changed().await;
    assert_eq!(h.daemon.config().alerts.warning_percent, 55.0);

    // A broken file keeps the settings that already work.
    std::fs::write(&h.paths.config_file, "[alerts\n").unwrap();
    h.daemon.reload_config_if_changed().await;
    assert_eq!(h.daemon.config().alerts.warning_percent, 55.0);
}

#[tokio::test]
async fn a_dropped_statusline_is_ingested_once() {
    let h = harness(10.0);
    let drop_box = h.paths.statusline_drop();
    drop_at(&drop_box, r#"{"model":{"display_name":"Opus 5.5"}}"#, NOW);

    h.daemon.ingest_statusline_file().await;
    h.daemon.ingest_statusline_file().await;
    assert_eq!(h.claude.ingested.lock().unwrap().len(), 1);
}

/// Write a drop box whose mtime is `at`, the way a real statusline run would.
fn drop_at(path: &std::path::Path, payload: &str, at: i64) {
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("run dir");
    std::fs::write(path, payload).expect("drop box written");
    let stamp = std::fs::FileTimes::new().set_modified(
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(at.try_into().expect("a date")),
    );
    std::fs::File::options()
        .write(true)
        .open(path)
        .expect("drop box opened")
        .set_times(stamp)
        .expect("mtime set");
}

#[tokio::test]
async fn a_statusline_dropped_long_ago_is_ignored() {
    let h = harness(10.0);
    let drop_box = h.paths.statusline_drop();
    // Left behind by a session that ended yesterday: stamping it `now` would let
    // it override whatever the OAuth reader has just produced.
    drop_at(
        &drop_box,
        r#"{"model":{"display_name":"Opus 5.5"}}"#,
        NOW - 86_400,
    );

    h.daemon.ingest_statusline_file().await;
    assert!(h.claude.ingested.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_dropped_statusline_is_observed_at_the_files_mtime() {
    let h = harness(10.0);
    let dropped_at = NOW - 120;
    drop_at(
        &h.paths.statusline_drop(),
        r#"{"model":{"display_name":"Opus 5.5"}}"#,
        dropped_at,
    );

    h.daemon.ingest_statusline_file().await;
    assert_eq!(
        *h.claude.observed_at.lock().unwrap(),
        vec![dropped_at],
        "the mtime is the observation time, not now()"
    );
}

#[tokio::test]
async fn statusline_ingest_reports_a_rejected_payload() {
    let h = harness(10.0);
    let error = h
        .daemon
        .ingest_claude_statusline("bad payload")
        .await
        .unwrap_err();
    assert_eq!(error, "unrecognised statusline");
    h.daemon
        .ingest_claude_statusline(r#"{"ok":1}"#)
        .await
        .expect("accepted");
    assert_eq!(h.claude.ingested.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn history_is_pruned_at_most_once_a_day() {
    let h = harness(42.0);
    h.daemon.refresh_all(true).await;
    h.daemon.prune_history().await;
    // Retention is 35 days, so today's sample survives.
    assert_eq!(
        h.daemon.history(ProviderId::Claude, "session", 0).await,
        vec![(NOW, 42.0)]
    );
    h.daemon.prune_history().await;
}

#[tokio::test]
async fn settings_expose_the_current_config_as_json() {
    let h = harness(10.0);
    let json = h.daemon.settings_json();
    assert_eq!(Config::from_json(&json).unwrap(), Config::default());
}

#[tokio::test]
async fn history_for_an_unknown_window_is_empty() {
    let h = harness(10.0);
    h.daemon.refresh_all(true).await;
    assert!(
        h.daemon
            .history(ProviderId::Claude, "made-up", 0)
            .await
            .is_empty()
    );
}

#[tokio::test(start_paused = true)]
async fn the_scheduler_drives_both_refresh_loops() {
    use std::time::Duration;
    use token_station::daemon::scheduler;

    /// Let the spawned loops run until `ready`, or give up.
    async fn settle(ready: impl Fn() -> bool) -> bool {
        for _ in 0..200 {
            if ready() {
                return true;
            }
            tokio::task::yield_now().await;
        }
        ready()
    }

    let h = harness(5.0);
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let handles = scheduler::spawn_all(&h.daemon, &shutdown_rx);
    // Let every loop reach its first sleep before moving the clock.
    settle(|| false).await;

    // Past one tokens interval (60 s) but not the limits interval (300 s).
    tokio::time::advance(Duration::from_secs(61)).await;
    let tokens = &h.claude.token_refreshes;
    assert!(settle(|| tokens.load(Ordering::SeqCst) >= 1).await);
    assert_eq!(h.claude.limits_refreshes.load(Ordering::SeqCst), 0);

    tokio::time::advance(Duration::from_secs(300)).await;
    let limits = &h.claude.limits_refreshes;
    assert!(settle(|| limits.load(Ordering::SeqCst) >= 1).await);

    shutdown_tx.send(true).expect("shutdown delivered");
    settle(|| false).await;
    for handle in handles {
        handle.abort();
    }
}

#[tokio::test]
async fn a_disabled_provider_is_rebuilt_and_reported_as_off() {
    let h = harness(10.0);
    h.daemon
        .set_settings(r#"{"codex":{"enabled":false}}"#)
        .await
        .expect("accepted");
    let snapshot = h.daemon.publisher.snapshot();
    let codex = snapshot
        .providers
        .iter()
        .find(|p| p.id == ProviderId::Codex)
        .expect("codex is present");
    assert_eq!(codex.state, ProviderState::Disabled);
    assert!(
        snapshot
            .meter
            .bars
            .iter()
            .all(|b| b.provider != ProviderId::Codex),
        "disabled providers get no meter bar"
    );
}

#[tokio::test]
async fn set_settings_refuses_a_config_file_it_must_not_rewrite() {
    let h = harness(10.0);
    // What home-manager leaves at `config.toml`: a symlink into the store.
    let target = h.paths.config_file.with_file_name("store-config.toml");
    std::fs::create_dir_all(h.paths.config_file.parent().unwrap()).unwrap();
    std::fs::write(&target, "[alerts]\nwarning_percent = 80.0\n").unwrap();
    std::os::unix::fs::symlink(&target, &h.paths.config_file).unwrap();

    let problems = h
        .daemon
        .set_settings(r#"{"alerts":{"warning_percent":60.0,"critical_percent":90.0}}"#)
        .await
        .unwrap_err();

    assert_eq!(problems, vec![settings::MANAGED_DECLARATIVELY.to_string()]);
    // Neither the link nor the file it points at moved, and nothing was applied.
    assert!(
        std::fs::symlink_metadata(&h.paths.config_file)
            .unwrap()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "[alerts]\nwarning_percent = 80.0\n"
    );
    assert_eq!(h.daemon.config(), Config::default());
}

#[tokio::test]
async fn a_switched_config_symlink_is_picked_up_even_with_an_unchanged_mtime() {
    let h = harness(10.0);
    let dir = h.paths.config_file.parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&dir).unwrap();

    // Two generations with the same whole-second mtime, as /nix/store has.
    let stamp = std::fs::FileTimes::new()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1));
    let write_generation = |name: &str, percent: &str| {
        let path = dir.join(name);
        std::fs::write(&path, format!("[alerts]\nwarning_percent = {percent}\n")).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(stamp)
            .unwrap();
        path
    };
    let first = write_generation("generation-1.toml", "55.0");
    let second = write_generation("generation-2.toml", "45.0");

    std::os::unix::fs::symlink(&first, &h.paths.config_file).unwrap();
    h.daemon.reload_config_if_changed().await;
    assert_eq!(h.daemon.config().alerts.warning_percent, 55.0);

    std::fs::remove_file(&h.paths.config_file).unwrap();
    std::os::unix::fs::symlink(&second, &h.paths.config_file).unwrap();
    h.daemon.reload_config_if_changed().await;
    assert_eq!(
        h.daemon.config().alerts.warning_percent,
        45.0,
        "the switched link is a change even though both mtimes read 1"
    );
}
