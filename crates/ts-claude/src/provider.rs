//! Glues the pure logic in [`crate::state`] to real IO: HTTP, the `claude`
//! binary and local logs.

use std::path::PathBuf;
use std::sync::{Mutex, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use ts_core::config::ClaudeConfig;
use ts_core::pricing::SharedPricing;
use ts_core::provider::{Ingest, IngestError, Provider, RefreshOutcome};
use ts_core::snapshot::{BreakdownRow, Credits, ProviderId, ProviderSnapshot, UsageWindow};
use ts_core::tokens::TokenLedger;

use crate::credentials::{self, Credentials};
use crate::env::{self, ClaudeEnv};
use crate::labels;
use crate::logs::{self, LogScanState};
use crate::oauth_usage::{self, FetchOutcome};
use crate::state::{self, Decision, LastOutcome, LimitsState, StateInputs};
use crate::statusline;

const AUTH_STATUS_TIMEOUT: Duration = Duration::from_secs(20);
const AUTH_STATUS_MIN_INTERVAL_SECS: i64 = 600;
const STATUSLINE_FRESH_SECS: i64 = 600;
const TOKEN_RETENTION_SECS: i64 = 8 * 86_400;
const FALLBACK_USER_AGENT: &str = "claude-code/2.1.283";

#[derive(Debug, Clone, Default)]
struct Observations {
    endpoint_windows: Vec<UsageWindow>,
    statusline_session: Option<UsageWindow>,
    statusline_weekly_all: Option<UsageWindow>,
    credits: Option<Credits>,
    breakdown: Vec<BreakdownRow>,
    plan: Option<String>,
    last_endpoint_success_at: Option<i64>,
}

/// Filesystem/process facts `snapshot` needs but must never fetch itself.
/// Refreshed from `refresh_limits`/`refresh_tokens`; read as a plain cache.
#[derive(Debug, Clone, Default)]
struct IoCache {
    claude_binary_found: bool,
    credentials_file_exists: bool,
    /// Whether the credentials on disk are expired, independent of whether
    /// we still have window data from another source.
    credentials_expired: bool,
    projects_dir_exists: bool,
}

pub struct ClaudeProvider {
    config: ClaudeConfig,
    pricing: SharedPricing,
    env: ClaudeEnv,
    http: reqwest::Client,
    // Lock order when more than one is held at once: `limits`, then
    // `observations`, then `io_cache`. Never acquire them in a different
    // order, or two call paths taking them in opposite orders can deadlock.
    limits: RwLock<LimitsState>,
    observations: RwLock<Observations>,
    io_cache: RwLock<IoCache>,
    ledger: Mutex<TokenLedger>,
    scan_state: Mutex<LogScanState>,
    version_cache: Mutex<Option<String>>,
    binary_cache: Mutex<Option<Option<PathBuf>>>,
}

impl ClaudeProvider {
    /// Provider using the real process environment (`HOME`, `CLAUDE_CONFIG_DIR`, `PATH`).
    pub fn new(config: ClaudeConfig, pricing: SharedPricing) -> Self {
        ClaudeProvider::with_env(config, pricing, ClaudeEnv::current())
    }

    /// Provider using an injected environment, for tests.
    pub fn with_env(config: ClaudeConfig, pricing: SharedPricing, env: ClaudeEnv) -> Self {
        let http = reqwest::Client::builder()
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        let provider = ClaudeProvider {
            config,
            pricing,
            env,
            http,
            limits: RwLock::new(LimitsState::default()),
            observations: RwLock::new(Observations::default()),
            io_cache: RwLock::new(IoCache::default()),
            ledger: Mutex::new(TokenLedger::new()),
            scan_state: Mutex::new(LogScanState::default()),
            version_cache: Mutex::new(None),
            binary_cache: Mutex::new(None),
        };
        // Populate the IO cache once up front so a `snapshot()` taken before
        // the first `refresh_limits`/`refresh_tokens` tick still reflects
        // reality instead of the all-`false` default.
        provider.refresh_io_cache(unix_now());
        provider
    }

    fn config_dir(&self) -> PathBuf {
        env::config_dir(&self.config.config_dir, &self.env)
    }

    fn credentials_path(&self) -> PathBuf {
        self.config_dir().join(".credentials.json")
    }

    fn projects_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = vec![self.config_dir().join("projects")];
        let alt = self.env.home.join(".config/claude/projects");
        if alt.exists() {
            dirs.push(alt);
        }
        dirs
    }

    fn claude_binary(&self) -> Option<PathBuf> {
        let mut cache = self.binary_cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cached) = cache.as_ref() {
            return cached.clone();
        }
        let found = env::find_claude_binary(&self.config.binary, &self.env);
        *cache = Some(found.clone());
        found
    }

    async fn user_agent(&self) -> String {
        if !self.config.user_agent.trim().is_empty() {
            return self.config.user_agent.clone();
        }
        {
            let cached = self.version_cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ua) = cached.as_ref() {
                return ua.clone();
            }
        }
        let ua = match self.detect_version().await {
            Some(v) => format!("claude-code/{v}"),
            None => FALLBACK_USER_AGENT.to_string(),
        };
        *self.version_cache.lock().unwrap_or_else(|e| e.into_inner()) = Some(ua.clone());
        ua
    }

    async fn detect_version(&self) -> Option<String> {
        let bin = self.claude_binary()?;
        let mut cmd = tokio::process::Command::new(bin);
        cmd.arg("--version").kill_on_drop(true);
        let output = tokio::time::timeout(AUTH_STATUS_TIMEOUT, cmd.output())
            .await
            .ok()?
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .next()
            .map(str::to_string)
    }

    /// Run `claude auth status --json` as the only allowed refresh attempt;
    /// its stdout is never inspected (it may contain PII, and it carries no
    /// data we need beyond "did the CLI refresh its token").
    async fn run_auth_status(&self) {
        let Some(bin) = self.claude_binary() else {
            return;
        };
        let mut cmd = tokio::process::Command::new(bin);
        cmd.arg("auth")
            .arg("status")
            .arg("--json")
            .kill_on_drop(true);
        let _ = tokio::time::timeout(AUTH_STATUS_TIMEOUT, cmd.output()).await;
    }

    fn should_check_auth_status(&self, now: i64) -> bool {
        let st = self.limits.read().unwrap_or_else(|e| e.into_inner());
        st.last_auth_status_check
            .is_none_or(|t| now - t >= AUTH_STATUS_MIN_INTERVAL_SECS)
    }

    fn mark_auth_status_checked(&self, now: i64) {
        self.limits
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .last_auth_status_check = Some(now);
    }

    /// Recompute the filesystem/process facts `snapshot` needs, without ever
    /// holding a lock while doing IO: everything below is plain reads/stats
    /// into locals, and only the final assignment touches `io_cache`.
    fn refresh_io_cache(&self, now: i64) {
        let claude_binary_found = self.claude_binary().is_some();
        let creds_path = self.credentials_path();
        let credentials_file_exists = creds_path.exists();
        let credentials_expired = credentials_file_exists
            && credentials::load(&creds_path)
                .map(|c| c.expired(now))
                .unwrap_or(false);
        let projects_dir_exists = self.projects_dirs().iter().any(|p| p.exists());
        *self.io_cache.write().unwrap_or_else(|e| e.into_inner()) = IoCache {
            claude_binary_found,
            credentials_file_exists,
            credentials_expired,
            projects_dir_exists,
        };
    }

    fn set_last_outcome(&self, outcome: LastOutcome) {
        self.limits
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .last_outcome = outcome;
    }

    fn reset_backoff(&self) {
        let mut st = self.limits.write().unwrap_or_else(|e| e.into_inner());
        st.backoff_until = None;
        st.backoff_current_secs = 0;
    }

    fn disable_endpoint(&self, message: String) {
        let mut st = self.limits.write().unwrap_or_else(|e| e.into_inner());
        st.endpoint_disabled = true;
        st.disabled_message = Some(message);
    }

    fn apply_backoff(&self, retry_after_secs: Option<u64>, now: i64) {
        let mut st = self.limits.write().unwrap_or_else(|e| e.into_inner());
        let (until, current) = state::backoff_after_rate_limit(
            st.backoff_current_secs,
            retry_after_secs,
            self.config.min_endpoint_interval_secs,
            now,
        );
        st.backoff_until = Some(until);
        st.backoff_current_secs = current;
    }

    fn store_parsed(&self, parsed: oauth_usage::ParsedUsage, creds: &Credentials, now: i64) {
        let mut obs = self.observations.write().unwrap_or_else(|e| e.into_inner());
        obs.endpoint_windows = parsed.windows;
        obs.credits = parsed.credits;
        obs.breakdown = parsed.breakdown;
        obs.plan = Some(labels::plan_label(
            &creds.rate_limit_tier,
            &creds.subscription_type,
        ));
        obs.last_endpoint_success_at = Some(now);
    }

    async fn refresh_expired_credentials(
        &self,
        creds_path: &std::path::Path,
        now: i64,
    ) -> Option<Credentials> {
        if self.should_check_auth_status(now) {
            self.run_auth_status().await;
            self.mark_auth_status_checked(now);
        }
        credentials::load(creds_path).ok()
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

const SIGN_IN_EXPIRED_MESSAGE: &str = "Sign-in expired. Open Claude Code to refresh it.";
const NOT_SIGNED_IN_MESSAGE: &str = "Not signed in to Claude Code. Run `claude` and log in.";

#[async_trait]
impl Provider for ClaudeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Claude
    }

    async fn refresh_limits(&self, force: bool) -> RefreshOutcome {
        let now = unix_now();
        self.refresh_io_cache(now);
        // Check-and-record happens under one write lock so two concurrent
        // callers can never both observe `Proceed` for the same interval.
        let decision = {
            let mut st = self.limits.write().unwrap_or_else(|e| e.into_inner());
            state::decide(&mut st, &self.config, now, force)
        };
        match decision {
            Decision::Proceed => {}
            Decision::SoftSkip(msg) => return RefreshOutcome::Skipped(msg),
            Decision::Skip(msg) => {
                self.set_last_outcome(LastOutcome::Skipped(msg.clone()));
                return RefreshOutcome::Skipped(msg);
            }
        }

        let creds_path = self.credentials_path();
        let mut creds = match credentials::load(&creds_path) {
            Ok(c) => c,
            Err(_) => {
                self.set_last_outcome(LastOutcome::Skipped(NOT_SIGNED_IN_MESSAGE.to_string()));
                return RefreshOutcome::Skipped(NOT_SIGNED_IN_MESSAGE.to_string());
            }
        };

        if creds.expired(now) {
            if let Some(refreshed) = self.refresh_expired_credentials(&creds_path, now).await {
                creds = refreshed;
            }
            if creds.expired(now) {
                self.set_last_outcome(LastOutcome::Skipped(SIGN_IN_EXPIRED_MESSAGE.to_string()));
                return RefreshOutcome::Skipped(SIGN_IN_EXPIRED_MESSAGE.to_string());
            }
        }

        let user_agent = self.user_agent().await;
        let outcome = oauth_usage::fetch(
            &self.http,
            &self.env.api_base_url,
            &creds.access_token,
            &user_agent,
            now,
        )
        .await;
        match outcome {
            FetchOutcome::Ok(body) => match oauth_usage::parse_usage(&body, now) {
                Ok(parsed) => {
                    self.store_parsed(parsed, &creds, now);
                    self.reset_backoff();
                    self.set_last_outcome(LastOutcome::Ok);
                    RefreshOutcome::Updated
                }
                Err(e) => {
                    let msg = format!("Claude usage response was unreadable: {e}");
                    self.set_last_outcome(LastOutcome::Failed(msg.clone()));
                    RefreshOutcome::Failed(msg)
                }
            },
            FetchOutcome::Unauthorized => {
                let _ = self.refresh_expired_credentials(&creds_path, now).await;
                self.set_last_outcome(LastOutcome::Skipped(SIGN_IN_EXPIRED_MESSAGE.to_string()));
                RefreshOutcome::Skipped(SIGN_IN_EXPIRED_MESSAGE.to_string())
            }
            FetchOutcome::Forbidden => {
                let msg = "Claude sign-in lacks the profile scope; using statusline data only."
                    .to_string();
                self.disable_endpoint(msg.clone());
                self.set_last_outcome(LastOutcome::Failed(msg.clone()));
                RefreshOutcome::Failed(msg)
            }
            FetchOutcome::RateLimited { retry_after_secs } => {
                self.apply_backoff(retry_after_secs, now);
                let msg = "Rate limited by Claude's usage endpoint.".to_string();
                self.set_last_outcome(LastOutcome::Failed(msg.clone()));
                RefreshOutcome::Failed(msg)
            }
            FetchOutcome::ServerError(code) => {
                let msg = format!("Claude usage endpoint returned HTTP {code}.");
                self.set_last_outcome(LastOutcome::Failed(msg.clone()));
                RefreshOutcome::Failed(msg)
            }
            FetchOutcome::Network(e) => {
                let msg = format!("Could not reach Claude's usage endpoint: {e}");
                self.set_last_outcome(LastOutcome::Failed(msg.clone()));
                RefreshOutcome::Failed(msg)
            }
        }
    }

    async fn refresh_tokens(&self) {
        let roots = self.projects_dirs();
        let now = unix_now();
        self.refresh_io_cache(now);
        let mut state = {
            let mut guard = self.scan_state.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *guard)
        };
        let (events, new_state) = tokio::task::spawn_blocking(move || {
            let events = logs::scan(&roots, &mut state, now);
            (events, state)
        })
        .await
        .unwrap_or_else(|_| (Vec::new(), LogScanState::default()));

        *self.scan_state.lock().unwrap_or_else(|e| e.into_inner()) = new_state;

        if events.is_empty() {
            return;
        }
        let cutoff = now - TOKEN_RETENTION_SECS;
        let mut guard = self.ledger.lock().unwrap_or_else(|e| e.into_inner());
        let ledger = std::mem::take(&mut *guard);
        *guard = ledger.with_events(events).pruned(cutoff);
    }

    fn snapshot(&self, now: i64) -> ProviderSnapshot {
        // Fixed lock order (see the field comment on `ClaudeProvider`): never
        // acquire these in a different order elsewhere.
        let limits = self.limits.read().unwrap_or_else(|e| e.into_inner());
        let obs = self.observations.read().unwrap_or_else(|e| e.into_inner());
        let io = self.io_cache.read().unwrap_or_else(|e| e.into_inner());

        let windows = state::merge_windows(
            &obs.endpoint_windows,
            obs.statusline_session.as_ref(),
            obs.statusline_weekly_all.as_ref(),
            now,
        );
        let has_window_data = !windows.is_empty();
        let has_fresh_statusline = [
            obs.statusline_session.as_ref(),
            obs.statusline_weekly_all.as_ref(),
        ]
        .into_iter()
        .flatten()
        .map(|w| w.observed_at)
        .max()
        .is_some_and(|t| now - t < STATUSLINE_FRESH_SECS);

        // No IO here: `io` is refreshed by `refresh_limits`/`refresh_tokens`.
        let credentials_expired_with_no_data =
            io.credentials_file_exists && !has_window_data && io.credentials_expired;

        let inputs = StateInputs {
            claude_binary_found: io.claude_binary_found,
            credentials_file_exists: io.credentials_file_exists,
            credentials_expired_with_no_data,
            last_outcome: limits.last_outcome.clone(),
            has_window_data,
            has_fresh_statusline,
            projects_dir_exists: io.projects_dir_exists,
        };
        let (provider_state, message) = state::provider_state(&inputs);

        let mut snap = ProviderSnapshot::empty(ProviderId::Claude, provider_state);
        snap.message = message;
        snap.plan = obs.plan.clone();
        snap.windows = windows;
        snap.credits = obs.credits.clone();
        snap.breakdown = obs.breakdown.clone();
        snap.updated_at = obs.last_endpoint_success_at;

        let ledger = self.ledger.lock().unwrap_or_else(|e| e.into_inner());
        if !ledger.is_empty() {
            let now_dt = chrono::DateTime::from_timestamp(now, 0).unwrap_or_else(chrono::Utc::now);
            let pricing = self.pricing.read().unwrap_or_else(|e| e.into_inner());
            snap.tokens = Some(ledger.report(now_dt, &chrono::Local, &pricing));
        }
        snap
    }

    fn next_limits_refresh(&self, now: i64, default_interval: Duration) -> Duration {
        let limits = self.limits.read().unwrap_or_else(|e| e.into_inner());
        let obs = self.observations.read().unwrap_or_else(|e| e.into_inner());
        let resets = obs.endpoint_windows.iter().filter_map(|w| w.resets_at);
        state::next_limits_refresh(
            &limits,
            self.config.min_endpoint_interval_secs,
            now,
            default_interval,
            resets,
        )
    }

    fn ingest(&self, payload: Ingest, now: i64) -> Result<(), IngestError> {
        let Ingest::ClaudeStatusline(json) = payload;
        match statusline::parse(&json, now) {
            Ok(Some(observation)) => {
                let mut obs = self.observations.write().unwrap_or_else(|e| e.into_inner());
                if let Some(w) = observation.session {
                    obs.statusline_session = Some(w);
                }
                if let Some(w) = observation.weekly_all {
                    obs.statusline_weekly_all = Some(w);
                }
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(e) => Err(IngestError::Invalid(e)),
        }
    }
}
