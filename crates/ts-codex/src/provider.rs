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

/// Everything [`refresh_limits`](CodexProvider::refresh_limits) and
/// [`refresh_tokens`](CodexProvider::refresh_tokens) mutate. Kept behind one
/// async mutex; [`CodexProvider::snapshot`] never touches it.
struct Inner {
    session: Option<Session>,
    process_backoff: Backoff,
    limits_backoff: Backoff,
    scanner: RolloutScanner,
    last_rollout_rate_limits: Option<(i64, RateLimitSnapshotDto)>,
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
                scanner: RolloutScanner::new(),
                last_rollout_rate_limits: None,
            })),
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
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
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
}

fn unauthenticated_payload(message: String) -> Payload {
    Payload {
        state: ProviderState::Unauthenticated,
        message: Some(message),
        plan: None,
        windows: Vec::new(),
        credits: None,
        account_tokens: None,
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
                .unwrap()
                .backoff_resume_at
                .map(|t| now >= t)
                .unwrap_or(true);
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
        let resume_at = match &attempt {
            Ok(_) => {
                inner.limits_backoff.succeed();
                None
            }
            Err(_) => {
                inner.limits_backoff.fail(now);
                Some(now + inner.limits_backoff.remaining(now).as_secs() as i64)
            }
        };
        drop(inner);
        self.schedule_idle_shutdown();
        self.scheduling.write().unwrap().backoff_resume_at = resume_at;

        match attempt {
            Ok(payload) => {
                self.publish(Cached {
                    state: payload.state,
                    message: payload.message,
                    plan: payload.plan,
                    windows: payload.windows,
                    credits: payload.credits,
                    account_tokens: payload.account_tokens,
                    updated_at: Some(now),
                });
                RefreshOutcome::Updated
            }
            Err(message) => {
                let mut cached = self.cached.read().unwrap().clone();
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
        let scanner = {
            let mut inner = self.inner.lock().await;
            std::mem::take(&mut inner.scanner)
        };
        let (scanner, output) = tokio::task::spawn_blocking(move || {
            let mut scanner = scanner;
            let output = scanner.scan(&homes, now);
            (scanner, output)
        })
        .await
        .unwrap_or_else(|_| {
            (
                RolloutScanner::new(),
                crate::rollouts::ScanOutput::default(),
            )
        });

        {
            let mut inner = self.inner.lock().await;
            inner.scanner = scanner;
            if let Some(latest) = output.latest_rate_limits {
                inner.last_rollout_rate_limits = Some(latest);
            }
        }
        let mut ledger = self.ledger.write().unwrap();
        *ledger = std::mem::take(&mut *ledger)
            .with_events(output.events)
            .pruned(now - TOKEN_RETENTION_SECS);
    }

    fn snapshot(&self, now: i64) -> ProviderSnapshot {
        let cached = self.cached.read().unwrap();
        let ledger = self.ledger.read().unwrap();
        let pricing = self.pricing.read().unwrap();
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
        let info = *self.scheduling.read().unwrap();
        let mut delay = default_interval;
        if let Some(resume_at) = info.backoff_resume_at {
            if resume_at > now {
                delay = delay.max(Duration::from_secs((resume_at - now) as u64));
            }
        }
        if let Some(reset_at) = info.earliest_reset_at {
            let until = reset_at + 30 - now;
            if until > 0 {
                delay = delay.min(Duration::from_secs(until as u64));
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

    fn publish(&self, cached: Cached) {
        let earliest_reset_at = cached.windows.iter().filter_map(|w| w.resets_at).min();
        self.scheduling.write().unwrap().earliest_reset_at = earliest_reset_at;
        *self.cached.write().unwrap() = cached;
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

        if let Some((ts, snapshot)) = &inner.last_rollout_rate_limits {
            let limit_id = snapshot.limit_id.as_deref().unwrap_or("codex");
            let windows = map_rate_limit_snapshot(limit_id, snapshot, *ts, "rollout");
            if !windows.is_empty() {
                return Ok(Payload {
                    state: ProviderState::Ok,
                    message: Some(ROLLOUT_FALLBACK_MESSAGE.to_string()),
                    plan: snapshot.plan_type.as_deref().map(plan_label),
                    windows,
                    credits: None,
                    account_tokens: None,
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

    /// Call `method` on the current session, tearing the session down (and
    /// applying process backoff) on any non-definitive failure.
    async fn call_on_session(
        &self,
        inner: &mut Inner,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
        now: i64,
    ) -> Result<serde_json::Value, CallOutcome> {
        let result = inner
            .session
            .as_ref()
            .expect("ensure_session was called first")
            .process
            .client
            .call(method, params, timeout)
            .await;
        classify_call_result(result).inspect_err(|outcome| {
            if let CallOutcome::Failed(_) = outcome {
                inner.session = None;
                inner.process_backoff.fail(now);
            }
        })
    }

    async fn try_app_server(
        &self,
        inner: &mut Inner,
        binary: &std::path::Path,
        now: i64,
    ) -> Result<Payload, String> {
        self.ensure_session(inner, binary, now)?;
        let timeout = if inner.session.as_ref().unwrap().first_call_done {
            Duration::from_secs(15)
        } else {
            Duration::from_secs(30)
        };

        if !inner.session.as_ref().unwrap().handshaked {
            let params = serde_json::json!({
                "clientInfo": {
                    "name": "token-station",
                    "title": "Token Station",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            });
            match self
                .call_on_session(inner, "initialize", params, timeout, now)
                .await
            {
                Ok(_) => {}
                Err(CallOutcome::Unauthenticated(msg)) => return Ok(unauthenticated_payload(msg)),
                Err(CallOutcome::Failed(msg)) => return Err(msg),
            }
            inner
                .session
                .as_ref()
                .unwrap()
                .process
                .client
                .notify("initialized", serde_json::Value::Null);
            inner.session.as_mut().unwrap().handshaked = true;
        }

        let account_value = match self
            .call_on_session(inner, "account/read", serde_json::json!({}), timeout, now)
            .await
        {
            Ok(v) => v,
            Err(CallOutcome::Unauthenticated(msg)) => return Ok(unauthenticated_payload(msg)),
            Err(CallOutcome::Failed(msg)) => return Err(msg),
        };
        {
            let session = inner.session.as_mut().unwrap();
            session.last_used = Instant::now();
            session.first_call_done = true;
        }

        let account: AccountReadResult = serde_json::from_value(account_value).unwrap_or_default();
        if account.account.is_none() && account.requires_openai_auth {
            inner.process_backoff.succeed();
            return Ok(unauthenticated_payload(NOT_SIGNED_IN_MESSAGE.to_string()));
        }
        if matches!(account.account, Some(AccountInfo::ApiKey {})) {
            inner.process_backoff.succeed();
            return Ok(Payload {
                state: ProviderState::Ok,
                message: Some(API_KEY_MESSAGE.to_string()),
                plan: Some("API key".to_string()),
                windows: Vec::new(),
                credits: None,
                account_tokens: None,
            });
        }

        let limits_value = match self
            .call_on_session(
                inner,
                "account/rateLimits/read",
                serde_json::json!({"excludeResetCreditDetails": true}),
                Duration::from_secs(15),
                now,
            )
            .await
        {
            Ok(v) => v,
            Err(CallOutcome::Unauthenticated(msg)) => return Ok(unauthenticated_payload(msg)),
            Err(CallOutcome::Failed(msg)) => return Err(msg),
        };
        let limits: RateLimitsReadResult = serde_json::from_value(limits_value).unwrap_or_default();

        let usage_value = match self
            .call_on_session(
                inner,
                "account/usage/read",
                serde_json::json!({}),
                Duration::from_secs(15),
                now,
            )
            .await
        {
            Ok(v) => v,
            Err(CallOutcome::Unauthenticated(msg)) => return Ok(unauthenticated_payload(msg)),
            Err(CallOutcome::Failed(msg)) => return Err(msg),
        };
        let usage: UsageReadResult = serde_json::from_value(usage_value).unwrap_or_default();

        inner.process_backoff.succeed();
        let (windows, credits, limits_plan) = map_rate_limits(&limits, now);
        let account_plan = match &account.account {
            Some(AccountInfo::Chatgpt { plan_type: Some(p) }) => Some(plan_label(p)),
            _ => None,
        };
        let message = match limits.ordinary_usage_allowed {
            Some(false) => Some(USAGE_LIMIT_MESSAGE.to_string()),
            _ => None,
        };
        Ok(Payload {
            state: ProviderState::Ok,
            message,
            plan: limits_plan.or(account_plan),
            windows,
            credits,
            account_tokens: Some(map_account_tokens(
                &usage,
                DateTime::<Utc>::from_timestamp(now, 0).unwrap_or_else(Utc::now),
                &Local,
            )),
        })
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
            );
            return match fallback.fetch_usage(&tokens, now).await {
                Ok(mapping) => Ok(Payload {
                    state: ProviderState::Ok,
                    message: None,
                    plan: mapping.plan,
                    windows: mapping.windows,
                    credits: mapping.credits,
                    account_tokens: None,
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
