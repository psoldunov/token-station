//! Orchestrates the app-server, HTTP fallback and rollout scanner behind the
//! [`ts_core::Provider`] trait.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock as StdRwLock;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Local, Utc};
use tokio::sync::Mutex as TokioMutex;
use ts_core::config::{CodexConfig, CodexProcessMode};
use ts_core::discovery::expand_tilde;
use ts_core::pricing::SharedPricing;
use ts_core::snapshot::{
    AccountTokens, Credits, ProviderId, ProviderSnapshot, ProviderState, UsageWindow,
};
use ts_core::tokens::TokenLedger;
use ts_core::{Provider, RefreshOutcome};

use crate::account_tokens::map_account_tokens;
use crate::app_server::{self, AppServerProcess, RpcCallError};
use crate::auth::read_auth;
use crate::backoff::Backoff;
use crate::dto::{
    AccountInfo, AccountReadResult, RateLimitSnapshotDto, RateLimitsReadResult, UsageReadResult,
};
use crate::env::CodexEnv;
use crate::http_fallback::{HttpFallback, HttpFallbackError};
use crate::rollouts::RolloutScanner;
use crate::windows::{map_rate_limit_snapshot, map_rate_limits, plan_label};

const TOKEN_RETENTION_SECS: i64 = 8 * 86_400;
const NOT_SIGNED_IN_MESSAGE: &str = "Codex is not signed in. Run `codex login`.";
const API_KEY_MESSAGE: &str = "Signed in with an API key: no plan limits.";
const USAGE_LIMIT_MESSAGE: &str = "Usage limit reached.";
const NOT_INSTALLED_MESSAGE: &str = "Codex is not installed.";
const ROLLOUT_FALLBACK_MESSAGE: &str =
    "Showing last known limits from local session logs (app-server and HTTP unavailable).";

/// A live app-server child plus the bookkeeping needed to reuse or retire it.
struct Session {
    process: AppServerProcess,
    handshaked: bool,
    first_call_done: bool,
    last_used: Instant,
}

/// Everything [`refresh_limits`](CodexProvider::refresh_limits) mutates. Kept
/// behind one async mutex; [`CodexProvider::snapshot`] never touches it.
///
/// The rollout `scanner` and the last rollout-derived rate limits deliberately
/// live outside this struct, behind locks of their own: they are
/// `refresh_tokens`'s only dependencies, and a slow `refresh_limits` attempt
/// (app-server call, HTTP fallback) must never block token scanning by holding
/// this lock the whole time.
struct Inner {
    session: Option<Session>,
    process_backoff: Backoff,
    limits_backoff: Backoff,
}

/// Published fields [`CodexProvider::snapshot`] reads without ever blocking.
#[derive(Clone)]
struct Cached {
    state: ProviderState,
    message: Option<String>,
    plan: Option<String>,
    windows: Vec<UsageWindow>,
    credits: Option<Credits>,
    account_tokens: Option<AccountTokens>,
    updated_at: Option<i64>,
}

impl Default for Cached {
    fn default() -> Cached {
        Cached {
            state: ProviderState::Loading,
            message: None,
            plan: None,
            windows: Vec::new(),
            credits: None,
            account_tokens: None,
            updated_at: None,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Scheduling {
    backoff_resume_at: Option<i64>,
    earliest_reset_at: Option<i64>,
}

pub struct CodexProvider {
    config: CodexConfig,
    pricing: SharedPricing,
    env: CodexEnv,
    inner: Arc<TokioMutex<Inner>>,
    /// Held across the whole blocking scan, so two scans cannot each start from
    /// a scanner with no state and re-read eight days of rollout files.
    scanner: Arc<TokioMutex<RolloutScanner>>,
    /// The newest rate-limit snapshot any rollout file has carried, with its
    /// timestamp. Its own lock: `refresh_tokens` must not queue behind a
    /// `refresh_limits` attempt just to store it.
    rollout_limits: StdRwLock<Option<(i64, RateLimitSnapshotDto)>>,
    cached: StdRwLock<Cached>,
    ledger: StdRwLock<TokenLedger>,
    scheduling: StdRwLock<Scheduling>,
}

impl CodexProvider {
    /// Provider using the real process environment (`HOME`, `CODEX_HOME`, `PATH`).
    pub fn new(config: CodexConfig, pricing: SharedPricing) -> CodexProvider {
        CodexProvider::with_env(config, pricing, CodexEnv::current())
    }

    /// Provider with every external dependency substituted; for tests.
    pub fn with_env(config: CodexConfig, pricing: SharedPricing, env: CodexEnv) -> CodexProvider {
        CodexProvider {
            config,
            pricing,
            env,
            inner: Arc::new(TokioMutex::new(Inner {
                session: None,
                process_backoff: Backoff::new(Duration::from_secs(5), Duration::from_secs(300)),
                limits_backoff: Backoff::new(Duration::from_secs(60), Duration::from_secs(1800)),
            })),
            scanner: Arc::new(TokioMutex::new(RolloutScanner::new())),
            rollout_limits: StdRwLock::new(None),
            cached: StdRwLock::new(Cached::default()),
            ledger: StdRwLock::new(TokenLedger::new()),
            scheduling: StdRwLock::new(Scheduling::default()),
        }
    }

    fn resolve_homes(&self) -> Vec<PathBuf> {
        if !self.config.homes.is_empty() {
            self.config
                .homes
                .iter()
                .map(|h| expand_tilde(h, &self.env.home))
                .collect()
        } else if let Some(codex_home) = &self.env.codex_home {
            vec![PathBuf::from(codex_home)]
        } else {
            vec![self.env.home.join(".codex")]
        }
    }

    fn resolve_binary(&self) -> Option<PathBuf> {
        if let Some(p) = &self.env.binary_override {
            return Some(p.clone());
        }
        ts_core::discovery::find_binary_in("codex", &self.config.binary, &self.env.search)
    }

    fn now_unix() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
    }
}

/// Everything one successful attempt (app-server, HTTP, or rollout fallback) produced.
struct Payload {
    state: ProviderState,
    message: Option<String>,
    plan: Option<String>,
    windows: Vec<UsageWindow>,
    credits: Option<Credits>,
    account_tokens: Option<AccountTokens>,
    /// When `Some`, the moment the underlying data was actually observed
    /// (used by the rollout fallback, whose data can be old); `None` means
    /// "fresh as of now", the caller's own timestamp.
    updated_at: Option<i64>,
}

fn unauthenticated_payload(message: String) -> Payload {
    Payload {
        state: ProviderState::Unauthenticated,
        message: Some(message),
        plan: None,
        windows: Vec::new(),
        credits: None,
        account_tokens: None,
        updated_at: None,
    }
}

/// What one app-server RPC call resolved to, once classified.
enum CallOutcome {
    /// A definitive answer: the app-server itself says we are not signed in.
    Unauthenticated(String),
    /// A retryable/unsupported failure; the caller should fall back.
    Failed(String),
}

fn classify_call_result(
    result: Result<serde_json::Value, RpcCallError>,
) -> Result<serde_json::Value, CallOutcome> {
    result.map_err(|error| match &error {
        RpcCallError::Remote(code, message)
            if *code == -32603 && looks_like_auth_error(message) =>
        {
            CallOutcome::Unauthenticated(format!("app-server auth error ({code}): {message}"))
        }
        RpcCallError::Remote(code, message) if *code == -32600 => CallOutcome::Failed(format!(
            "app-server does not support this method: {message}"
        )),
        RpcCallError::Remote(code, message) => {
            CallOutcome::Failed(format!("app-server error {code}: {message}"))
        }
        RpcCallError::Timeout => CallOutcome::Failed("app-server call timed out".to_string()),
        RpcCallError::Closed => CallOutcome::Failed("app-server connection closed".to_string()),
    })
}

/// An early exit from one `try_app_server` step: either a definitive payload
/// (e.g. the app-server says we're unauthenticated) or a hard failure that
/// aborts the whole attempt. Threading this through `?` keeps each step
/// function's own control flow small, which is what actually lowers
/// `try_app_server`'s complexity (splitting a function into pieces that all
/// still get inlined into one match does not).
enum StepOutcome {
    Payload(Box<Payload>),
    Error(String),
}

impl From<CallOutcome> for StepOutcome {
    fn from(outcome: CallOutcome) -> StepOutcome {
        match outcome {
            CallOutcome::Unauthenticated(msg) => {
                StepOutcome::Payload(Box::new(unauthenticated_payload(msg)))
            }
            CallOutcome::Failed(msg) => StepOutcome::Error(msg),
        }
    }
}

fn resolve_step(outcome: StepOutcome) -> Result<Payload, String> {
    match outcome {
        StepOutcome::Payload(payload) => Ok(*payload),
        StepOutcome::Error(message) => Err(message),
    }
}

fn api_key_payload() -> Payload {
    Payload {
        state: ProviderState::Ok,
        message: Some(API_KEY_MESSAGE.to_string()),
        plan: Some("API key".to_string()),
        windows: Vec::new(),
        credits: None,
        account_tokens: None,
        updated_at: None,
    }
}

fn build_ok_payload(
    limits: &RateLimitsReadResult,
    account: &AccountReadResult,
    account_tokens: Option<AccountTokens>,
    now: i64,
) -> Payload {
    let (windows, credits, limits_plan) = map_rate_limits(limits, now);
    let account_plan = match &account.account {
        Some(AccountInfo::Chatgpt { plan_type: Some(p) }) => Some(plan_label(p)),
        _ => None,
    };
    let message = match limits.ordinary_usage_allowed {
        Some(false) => Some(USAGE_LIMIT_MESSAGE.to_string()),
        _ => None,
    };
    Payload {
        state: ProviderState::Ok,
        message,
        plan: limits_plan.or(account_plan),
        windows,
        credits,
        account_tokens,
        updated_at: None,
    }
}

fn looks_like_auth_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    ["auth", "401", "unauthorized", "login"]
        .iter()
        .any(|needle| lower.contains(needle))
}

#[async_trait]
impl Provider for CodexProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Codex
    }

    async fn refresh_limits(&self, force: bool) -> RefreshOutcome {
        let now = Self::now_unix();
        if !force {
            let ready = self
                .scheduling
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .backoff_resume_at
                .map_or(true, |t| now >= t);
            if !ready {
                return RefreshOutcome::Skipped("backing off after a previous failure".into());
            }
        }

        let homes = self.resolve_homes();
        let binary = self.resolve_binary();
        if binary.is_none() && !homes.iter().any(|h| h.exists()) {
            self.publish(Cached {
                message: Some(NOT_INSTALLED_MESSAGE.to_string()),
                state: ProviderState::NotInstalled,
                ..Cached::default()
            });
            return RefreshOutcome::Skipped(NOT_INSTALLED_MESSAGE.into());
        }

        let mut inner = self.inner.lock().await;
        let attempt = self
            .attempt(&mut inner, binary.as_deref(), &homes, now)
            .await;
        let resume_at = if attempt.is_ok() {
            inner.limits_backoff.succeed();
            None
        } else {
            inner.limits_backoff.fail(now);
            let remaining =
                i64::try_from(inner.limits_backoff.remaining(now).as_secs()).unwrap_or(i64::MAX);
            Some(now.saturating_add(remaining))
        };
        drop(inner);
        self.schedule_idle_shutdown();
        self.scheduling
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .backoff_resume_at = resume_at;

        match attempt {
            Ok(payload) => {
                let updated_at = payload.updated_at.or(Some(now));
                self.publish(Cached {
                    state: payload.state,
                    message: payload.message,
                    plan: payload.plan,
                    windows: payload.windows,
                    credits: payload.credits,
                    account_tokens: payload.account_tokens,
                    updated_at,
                });
                RefreshOutcome::Updated
            }
            Err(message) => {
                let mut cached = self
                    .cached
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                cached.state = if cached.updated_at.is_some() {
                    ProviderState::Stale
                } else {
                    ProviderState::Error
                };
                cached.message = Some(message.clone());
                self.publish(cached);
                RefreshOutcome::Failed(message)
            }
        }
    }

    async fn refresh_tokens(&self) {
        let homes = self.resolve_homes();
        let now = Self::now_unix();
        // The guard is taken here and moved into the blocking task, so it is held
        // for the whole scan: taking the scanner out and putting it back would let
        // a second scan start from an empty one, re-read every rollout file of the
        // last eight days, and then overwrite the first scan's state.
        let mut guard = Arc::clone(&self.scanner).lock_owned().await;
        let output = tokio::task::spawn_blocking(move || guard.scan(&homes, now))
            .await
            .unwrap_or_default();

        // The ledger first: it is what this call exists for, and it needs no lock
        // anybody else holds for long.
        let mut ledger = self
            .ledger
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *ledger = std::mem::take(&mut *ledger)
            .with_events(output.events)
            .pruned(now - TOKEN_RETENTION_SECS);
        drop(ledger);

        if let Some(latest) = output.latest_rate_limits {
            self.keep_newer_rollout_limits(latest);
        }
    }

    fn snapshot(&self, now: i64) -> ProviderSnapshot {
        let cached = self
            .cached
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let ledger = self
            .ledger
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pricing = self
            .pricing
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tokens = if ledger.is_empty() {
            None
        } else {
            let now_dt = DateTime::<Utc>::from_timestamp(now, 0).unwrap_or_else(Utc::now);
            Some(ledger.report(now_dt, &Local, &pricing))
        };
        ProviderSnapshot {
            id: ProviderId::Codex,
            name: ProviderId::Codex.display_name().to_string(),
            state: cached.state,
            message: cached.message.clone(),
            plan: cached.plan.clone(),
            windows: cached.windows.clone(),
            credits: cached.credits.clone(),
            breakdown: Vec::new(),
            tokens,
            account_tokens: cached.account_tokens.clone(),
            updated_at: cached.updated_at,
        }
    }

    fn next_limits_refresh(&self, now: i64, default_interval: Duration) -> Duration {
        let info = *self
            .scheduling
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut delay = default_interval;
        if let Some(resume_at) = info.backoff_resume_at {
            if resume_at > now {
                // Guarded by `resume_at > now`, so the difference is positive.
                delay = delay.max(Duration::from_secs(
                    resume_at.saturating_sub(now).unsigned_abs(),
                ));
            }
        }
        if let Some(reset_at) = info.earliest_reset_at {
            let until = reset_at + 30 - now;
            if until > 0 {
                // Guarded by `until > 0`.
                delay = delay.min(Duration::from_secs(until.unsigned_abs()));
            }
        }
        delay.max(Duration::from_secs(30))
    }
}

impl CodexProvider {
    /// In on-demand mode, stop the app-server child once it has been idle for
    /// `linger_secs`: it holds ~150 MB while alive and a tray monitor polls
    /// only every few minutes.
    fn schedule_idle_shutdown(&self) {
        if self.config.process_mode != CodexProcessMode::OnDemand {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let inner = Arc::clone(&self.inner);
        let linger = Duration::from_secs(self.config.linger_secs);
        runtime.spawn(async move {
            tokio::time::sleep(linger).await;
            let mut guard = inner.lock().await;
            let idle = guard
                .session
                .as_ref()
                .is_some_and(|session| session.last_used.elapsed() >= linger);
            if idle {
                guard.session = None;
                tracing::debug!("stopped idle codex app-server");
            }
        });
    }

    /// Store `latest` unless what is already there was observed later.
    ///
    /// Two scans can finish out of order, and a rollout snapshot is only ever a
    /// fallback for when nothing live answers: the newest one is the only one
    /// worth keeping, so the older one is dropped rather than published as
    /// current.
    fn keep_newer_rollout_limits(&self, latest: (i64, RateLimitSnapshotDto)) {
        let mut slot = self
            .rollout_limits
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.as_ref().is_none_or(|(seen, _)| latest.0 >= *seen) {
            *slot = Some(latest);
        }
    }

    /// The newest rollout-derived snapshot, for the last-resort fallback.
    fn rollout_limits(&self) -> Option<(i64, RateLimitSnapshotDto)> {
        self.rollout_limits
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn publish(&self, cached: Cached) {
        let earliest_reset_at = cached.windows.iter().filter_map(|w| w.resets_at).min();
        self.scheduling
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .earliest_reset_at = earliest_reset_at;
        *self
            .cached
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = cached;
    }

    /// Try the app-server first (if a binary was found), then the HTTP
    /// fallback, then the last rollout-derived rate-limit snapshot.
    async fn attempt(
        &self,
        inner: &mut Inner,
        binary: Option<&std::path::Path>,
        homes: &[PathBuf],
        now: i64,
    ) -> Result<Payload, String> {
        let mut last_error = None;

        if let Some(binary) = binary {
            match self.try_app_server(inner, binary, now).await {
                Ok(payload) => return Ok(payload),
                Err(e) => last_error = Some(e),
            }
        }

        if self.config.http_fallback {
            match self.try_http_fallback(homes, now).await {
                Ok(payload) => return Ok(payload),
                Err(e) => last_error = Some(e),
            }
        }

        if let Some((ts, snapshot)) = &self.rollout_limits() {
            let limit_id = snapshot.limit_id.as_deref().unwrap_or("codex");
            let windows: Vec<UsageWindow> =
                map_rate_limit_snapshot(limit_id, snapshot, *ts, "rollout")
                    .into_iter()
                    .map(|w| crate::windows::apply_rollover(w, now))
                    .collect();
            if !windows.is_empty() {
                // This data can be arbitrarily old (the last time a rollout
                // line carried rate limits), so it is reported `Stale` with
                // its own timestamp, never `Ok` with `now`.
                return Ok(Payload {
                    state: ProviderState::Stale,
                    message: Some(ROLLOUT_FALLBACK_MESSAGE.to_string()),
                    plan: snapshot.plan_type.as_deref().map(plan_label),
                    windows,
                    credits: None,
                    account_tokens: None,
                    updated_at: Some(*ts),
                });
            }
        }

        Err(last_error.unwrap_or_else(|| "no usage source available".to_string()))
    }

    /// Ensure `inner.session` holds a usable, handshaken session, spawning or
    /// retiring the child as `process_mode` and `linger_secs` dictate.
    fn ensure_session(
        &self,
        inner: &mut Inner,
        binary: &std::path::Path,
        now: i64,
    ) -> Result<(), String> {
        let linger = Duration::from_secs(self.config.linger_secs);
        let stale = match &mut inner.session {
            Some(session) => match self.config.process_mode {
                CodexProcessMode::OnDemand => session.last_used.elapsed() > linger,
                CodexProcessMode::Persistent => {
                    matches!(session.process.child.try_wait(), Ok(Some(_)))
                }
            },
            None => false,
        };
        if stale {
            inner.session = None;
        }
        if inner.session.is_none() {
            if !inner.process_backoff.ready(now) {
                return Err("app-server is backing off after a recent failure".to_string());
            }
            match app_server::spawn(binary) {
                Ok(process) => {
                    inner.session = Some(Session {
                        process,
                        handshaked: false,
                        first_call_done: false,
                        last_used: Instant::now(),
                    });
                }
                Err(e) => {
                    inner.process_backoff.fail(now);
                    return Err(format!("cannot start codex app-server: {e}"));
                }
            }
        }
        Ok(())
    }

    /// Call `method` on the current session. The session (and process
    /// backoff) is only torn down on a transport-level failure (timeout or
    /// closed connection): a remote error *response* means the process is
    /// still alive and able to serve the next call, so tearing it down would
    /// throw away a perfectly good session over what might be one
    /// unsupported/optional method.
    async fn call_on_session(
        &self,
        inner: &mut Inner,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
        now: i64,
    ) -> Result<serde_json::Value, CallOutcome> {
        let Some(session) = inner.session.as_ref() else {
            return Err(CallOutcome::Failed(
                "app-server session missing (unexpected)".to_string(),
            ));
        };
        let result = session.process.client.call(method, params, timeout).await;
        if matches!(result, Err(RpcCallError::Timeout | RpcCallError::Closed)) {
            inner.session = None;
            inner.process_backoff.fail(now);
        }
        classify_call_result(result)
    }

    async fn try_app_server(
        &self,
        inner: &mut Inner,
        binary: &std::path::Path,
        now: i64,
    ) -> Result<Payload, String> {
        self.ensure_session(inner, binary, now)?;
        let Some(session) = inner.session.as_ref() else {
            return Err("app-server session missing right after ensure_session".to_string());
        };
        let timeout = if session.first_call_done {
            Duration::from_secs(15)
        } else {
            Duration::from_secs(30)
        };

        if let Err(outcome) = self.ensure_handshake(inner, timeout, now).await {
            return resolve_step(outcome);
        }
        let account = match self.fetch_account(inner, timeout, now).await {
            Ok(account) => account,
            Err(outcome) => return resolve_step(outcome),
        };
        if account.account.is_none() && account.requires_openai_auth {
            inner.process_backoff.succeed();
            return Ok(unauthenticated_payload(NOT_SIGNED_IN_MESSAGE.to_string()));
        }
        if matches!(account.account, Some(AccountInfo::ApiKey {})) {
            inner.process_backoff.succeed();
            return Ok(api_key_payload());
        }

        let limits = match self.fetch_rate_limits(inner, now).await {
            Ok(limits) => limits,
            Err(outcome) => return resolve_step(outcome),
        };
        // `account/usage/read` is optional: some app-server versions do not
        // implement it (-32600), and a failure or unreadable response here
        // must not discard the rate limits already fetched above.
        let account_tokens = match self.fetch_account_tokens(inner, now).await {
            Ok(tokens) => tokens,
            Err(outcome) => return resolve_step(outcome),
        };

        inner.process_backoff.succeed();
        Ok(build_ok_payload(&limits, &account, account_tokens, now))
    }

    /// Send `initialize`/`initialized` if the session hasn't handshaked yet.
    async fn ensure_handshake(
        &self,
        inner: &mut Inner,
        timeout: Duration,
        now: i64,
    ) -> Result<(), StepOutcome> {
        if inner.session.as_ref().is_some_and(|s| s.handshaked) {
            return Ok(());
        }
        let params = serde_json::json!({
            "clientInfo": {
                "name": "token-station",
                "title": "Token Station",
                "version": env!("CARGO_PKG_VERSION"),
            }
        });
        self.call_on_session(inner, "initialize", params, timeout, now)
            .await
            .map_err(StepOutcome::from)?;
        let Some(session) = inner.session.as_mut() else {
            return Err(StepOutcome::Error(
                "app-server session dropped mid-handshake".to_string(),
            ));
        };
        session
            .process
            .client
            .notify("initialized", serde_json::Value::Null);
        session.handshaked = true;
        Ok(())
    }

    async fn fetch_account(
        &self,
        inner: &mut Inner,
        timeout: Duration,
        now: i64,
    ) -> Result<AccountReadResult, StepOutcome> {
        let value = self
            .call_on_session(inner, "account/read", serde_json::json!({}), timeout, now)
            .await
            .map_err(StepOutcome::from)?;
        let Some(session) = inner.session.as_mut() else {
            return Err(StepOutcome::Error(
                "app-server session dropped after account/read".to_string(),
            ));
        };
        session.last_used = Instant::now();
        session.first_call_done = true;
        Ok(serde_json::from_value(value).unwrap_or_default())
    }

    async fn fetch_rate_limits(
        &self,
        inner: &mut Inner,
        now: i64,
    ) -> Result<RateLimitsReadResult, StepOutcome> {
        let value = self
            .call_on_session(
                inner,
                "account/rateLimits/read",
                serde_json::json!({"excludeResetCreditDetails": true}),
                Duration::from_secs(15),
                now,
            )
            .await
            .map_err(StepOutcome::from)?;
        // Core data: an unrecognized response shape must fail the attempt
        // rather than silently report zero windows via `unwrap_or_default`.
        serde_json::from_value(value).map_err(|e| {
            StepOutcome::Error(format!(
                "unexpected account/rateLimits/read response shape: {e}"
            ))
        })
    }

    async fn fetch_account_tokens(
        &self,
        inner: &mut Inner,
        now: i64,
    ) -> Result<Option<AccountTokens>, StepOutcome> {
        match self
            .call_on_session(
                inner,
                "account/usage/read",
                serde_json::json!({}),
                Duration::from_secs(15),
                now,
            )
            .await
        {
            Ok(v) => match serde_json::from_value::<UsageReadResult>(v) {
                Ok(usage) => Ok(Some(map_account_tokens(
                    &usage,
                    DateTime::<Utc>::from_timestamp(now, 0).unwrap_or_else(Utc::now),
                    &Local,
                ))),
                Err(e) => {
                    tracing::debug!(error = %e, "account/usage/read response shape unrecognized");
                    Ok(None)
                }
            },
            Err(CallOutcome::Unauthenticated(msg)) => {
                Err(StepOutcome::Payload(Box::new(unauthenticated_payload(msg))))
            }
            Err(CallOutcome::Failed(msg)) => {
                tracing::debug!(message = %msg, "account/usage/read unavailable; continuing without it");
                Ok(None)
            }
        }
    }

    async fn try_http_fallback(&self, homes: &[PathBuf], now: i64) -> Result<Payload, String> {
        let mut auth_error = None;
        for home in homes {
            let tokens = match read_auth(home) {
                Ok(tokens) => tokens,
                Err(e) => {
                    auth_error = Some(e.to_string());
                    continue;
                }
            };
            let fallback = HttpFallback::new(
                self.env.http_base_url.clone(),
                format!("token-station/{}", env!("CARGO_PKG_VERSION")),
                self.env.http_timeouts,
            );
            return match fallback.fetch_usage(&tokens, now).await {
                Ok(mapping) => Ok(Payload {
                    state: ProviderState::Ok,
                    message: None,
                    plan: mapping.plan,
                    windows: mapping.windows,
                    credits: mapping.credits,
                    account_tokens: None,
                    updated_at: None,
                }),
                Err(HttpFallbackError::Unauthorized) => {
                    Ok(unauthenticated_payload(NOT_SIGNED_IN_MESSAGE.to_string()))
                }
                Err(e) => Err(format!("HTTP fallback failed: {e}")),
            };
        }
        Err(auth_error.unwrap_or_else(|| "no codex home with auth.json found".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts_core::pricing::shared_bundled;

    fn provider() -> CodexProvider {
        CodexProvider::with_env(
            CodexConfig::default(),
            shared_bundled(),
            CodexEnv {
                home: PathBuf::from("/nonexistent"),
                codex_home: None,
                binary_override: None,
                http_base_url: "http://127.0.0.1:1".to_string(),
                http_timeouts: crate::http_fallback::HttpTimeouts::default(),
                search: ts_core::discovery::SearchEnv {
                    path: Some(String::new()),
                    home: PathBuf::from("/nonexistent"),
                    user: None,
                },
            },
        )
    }

    fn snapshot(limit_id: &str) -> RateLimitSnapshotDto {
        RateLimitSnapshotDto {
            limit_id: Some(limit_id.to_string()),
            ..RateLimitSnapshotDto::default()
        }
    }

    #[test]
    fn an_older_rollout_snapshot_never_replaces_a_newer_one() {
        let provider = provider();
        provider.keep_newer_rollout_limits((2_000, snapshot("new")));
        // Two scans can finish out of order; the stale one must not win.
        provider.keep_newer_rollout_limits((1_000, snapshot("old")));
        let (ts, kept) = provider.rollout_limits().expect("a snapshot is kept");
        assert_eq!(ts, 2_000);
        assert_eq!(kept.limit_id.as_deref(), Some("new"));

        // A later observation does replace it, even with the same timestamp: it
        // was read from the same file more recently.
        provider.keep_newer_rollout_limits((2_000, snapshot("same-time")));
        assert_eq!(
            provider.rollout_limits().and_then(|(_, s)| s.limit_id),
            Some("same-time".to_string())
        );
        provider.keep_newer_rollout_limits((3_000, snapshot("newest")));
        assert_eq!(provider.rollout_limits().map(|(ts, _)| ts), Some(3_000));
    }
}
