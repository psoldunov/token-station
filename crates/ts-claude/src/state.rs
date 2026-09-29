//! Pure decision logic: whether to refresh, how long to back off, how to merge
//! endpoint/statusline observations, and which [`ProviderState`] to report.
//!
//! Kept side-effect free so it can be unit tested without a clock or the
//! filesystem; [`crate::provider`] supplies real inputs.

use std::time::Duration;

use ts_core::config::ClaudeConfig;
use ts_core::snapshot::{ProviderState, UsageWindow};

/// Rate-limit backoff and soft-floor bookkeeping for the oauth endpoint.
#[derive(Debug, Clone, Default)]
pub struct LimitsState {
    pub last_attempt_at: Option<i64>,
    pub backoff_until: Option<i64>,
    pub backoff_current_secs: u64,
    pub endpoint_disabled: bool,
    pub disabled_message: Option<String>,
    pub last_auth_status_check: Option<i64>,
    pub last_outcome: LastOutcome,
}

const HARD_FLOOR_SECS: i64 = 30;
const BACKOFF_BASE_SECS: u64 = 300;
const BACKOFF_CAP_SECS: u64 = 3600;
/// Upper bound on a Claude-supplied `Retry-After` in seconds, so a corrupt or
/// hostile response cannot push `backoff_until` far enough to overflow `i64`
/// arithmetic (or just wedge the provider for an absurd length of time).
const MAX_RETRY_AFTER_SECS: i64 = 24 * 3600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Proceed,
    /// A real, persistent reason not to refresh (backoff, disabled, ...):
    /// worth recording as the provider's last outcome.
    Skip(String),
    /// The scheduler ticked inside the soft interval floor. Not recorded as
    /// the last outcome: it says nothing about data health and would flip a
    /// previously-`Ok` state to `Stale` on every fast tick.
    SoftSkip(String),
}

/// Whether `refresh_limits` should actually call the network. When this
/// returns [`Decision::Proceed`], `state.last_attempt_at` has already been set
/// to `now`, in the same lock acquisition the caller used to read `state`, so
/// two concurrent callers can never both observe `Proceed`.
pub fn decide(state: &mut LimitsState, config: &ClaudeConfig, now: i64, force: bool) -> Decision {
    if !config.use_oauth_endpoint {
        return Decision::Skip("Claude oauth usage endpoint is disabled in settings.".to_string());
    }
    if state.endpoint_disabled {
        return Decision::Skip(
            state
                .disabled_message
                .clone()
                .unwrap_or_else(|| "Claude oauth usage endpoint is disabled.".to_string()),
        );
    }
    if let Some(until) = state.backoff_until {
        if now < until {
            return Decision::Skip(format!(
                "Rate limited by Claude; retrying in {}s.",
                until - now
            ));
        }
    }
    if let Some(last) = state.last_attempt_at {
        let floor = if force {
            HARD_FLOOR_SECS
        } else {
            i64::try_from(config.min_endpoint_interval_secs)
                .unwrap_or(i64::MAX)
                .max(HARD_FLOOR_SECS)
        };
        if now - last < floor {
            return Decision::SoftSkip("Refreshed too recently.".to_string());
        }
    }
    state.last_attempt_at = Some(now);
    Decision::Proceed
}

/// Backoff after a 429. `retry_after_secs` comes from the response header when
/// present.
pub fn backoff_after_rate_limit(
    prior_current_secs: u64,
    retry_after_secs: Option<u64>,
    min_endpoint_interval_secs: u64,
    now: i64,
) -> (i64, u64) {
    if let Some(secs) = retry_after_secs {
        let clamped = i64::try_from(secs)
            .unwrap_or(i64::MAX)
            .min(MAX_RETRY_AFTER_SECS);
        (now.saturating_add(clamped), prior_current_secs)
    } else {
        let base = min_endpoint_interval_secs.max(BACKOFF_BASE_SECS);
        let secs = if prior_current_secs == 0 {
            base
        } else {
            (prior_current_secs * 2).min(BACKOFF_CAP_SECS)
        };
        (
            now.saturating_add(i64::try_from(secs).unwrap_or(i64::MAX)),
            secs,
        )
    }
}

/// Delay before the next `refresh_limits` attempt.
pub fn next_limits_refresh(
    state: &LimitsState,
    min_endpoint_interval_secs: u64,
    now: i64,
    default_interval: Duration,
    resets_at: impl Iterator<Item = i64>,
) -> Duration {
    let min_interval = i64::try_from(min_endpoint_interval_secs).unwrap_or(i64::MAX);
    let mandatory = [
        state.backoff_until.map_or(0, |u| (u - now).max(0)),
        state.last_attempt_at.map_or(0, |last| {
            last.saturating_add(min_interval).saturating_sub(now).max(0)
        }),
    ]
    .into_iter()
    .max()
    .unwrap_or(0);

    let default_secs = i64::try_from(default_interval.as_secs()).unwrap_or(i64::MAX);
    let earliest_reset = resets_at.filter(|&r| r > now).map(|r| r - now).min();
    let desired = match earliest_reset {
        Some(r) => default_secs.min(r + HARD_FLOOR_SECS),
        None => default_secs,
    };

    let secs = mandatory.max(desired).max(HARD_FLOOR_SECS);
    // `secs` is at least `HARD_FLOOR_SECS`, so this is exact.
    Duration::from_secs(secs.unsigned_abs())
}

/// Pick whichever of an endpoint/statusline observation is newer, apply
/// rollover, and combine with the endpoint's other windows.
pub fn merge_windows(
    endpoint: &[UsageWindow],
    statusline_session: Option<&UsageWindow>,
    statusline_weekly: Option<&UsageWindow>,
    now: i64,
) -> Vec<UsageWindow> {
    let mut out = Vec::new();
    let endpoint_session = endpoint.iter().find(|w| w.id == "session");
    let endpoint_weekly = endpoint.iter().find(|w| w.id == "weekly_all");

    if let Some(w) = newer(endpoint_session, statusline_session) {
        out.push(apply_rollover(w.clone(), now));
    }
    if let Some(w) = newer(endpoint_weekly, statusline_weekly) {
        out.push(apply_rollover(w.clone(), now));
    }
    for w in endpoint
        .iter()
        .filter(|w| w.id != "session" && w.id != "weekly_all")
    {
        out.push(apply_rollover(w.clone(), now));
    }
    out
}

fn newer<'a>(a: Option<&'a UsageWindow>, b: Option<&'a UsageWindow>) -> Option<&'a UsageWindow> {
    match (a, b) {
        (Some(x), Some(y)) => Some(if y.observed_at > x.observed_at { y } else { x }),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

fn apply_rollover(w: UsageWindow, now: i64) -> UsageWindow {
    match w.resets_at {
        Some(r) if r <= now => UsageWindow {
            used_percent: 0.0,
            resets_at: None,
            source: "rollover".to_string(),
            ..w
        },
        _ => w,
    }
}

/// The last thing a limits refresh attempt did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum LastOutcome {
    #[default]
    None,
    Ok,
    Skipped(String),
    Failed(String),
}

/// Everything [`provider_state`] needs, gathered from IO by the caller.
#[derive(Debug, Clone)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent observations the caller gathers separately; every combination is reachable, so no enum models them"
)]
pub struct StateInputs {
    pub claude_binary_found: bool,
    pub credentials_file_exists: bool,
    pub credentials_expired_with_no_data: bool,
    pub last_outcome: LastOutcome,
    pub has_window_data: bool,
    pub has_fresh_statusline: bool,
    pub projects_dir_exists: bool,
}

/// Classify current data health into a [`ProviderState`] plus optional message.
pub fn provider_state(inputs: &StateInputs) -> (ProviderState, Option<String>) {
    if !inputs.claude_binary_found && !inputs.credentials_file_exists && !inputs.projects_dir_exists
    {
        return (ProviderState::NotInstalled, None);
    }
    if !inputs.credentials_file_exists {
        return (
            ProviderState::Unauthenticated,
            Some("Not signed in to Claude Code. Run `claude` and log in.".to_string()),
        );
    }
    if inputs.credentials_expired_with_no_data {
        return (
            ProviderState::Unauthenticated,
            Some("Sign-in expired. Open Claude Code to refresh it.".to_string()),
        );
    }
    if inputs.has_fresh_statusline {
        return (ProviderState::Ok, None);
    }
    match &inputs.last_outcome {
        LastOutcome::Ok => (ProviderState::Ok, None),
        LastOutcome::None => {
            if inputs.has_window_data {
                (ProviderState::Stale, None)
            } else {
                (ProviderState::Loading, None)
            }
        }
        LastOutcome::Skipped(msg) => {
            if inputs.has_window_data {
                (ProviderState::Stale, Some(msg.clone()))
            } else {
                (ProviderState::Loading, Some(msg.clone()))
            }
        }
        LastOutcome::Failed(msg) => {
            if inputs.has_window_data {
                (ProviderState::Stale, Some(msg.clone()))
            } else {
                (ProviderState::Error, Some(msg.clone()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(
        id: &str,
        pct: f64,
        resets_at: Option<i64>,
        observed_at: i64,
        source: &str,
    ) -> UsageWindow {
        UsageWindow {
            id: id.to_string(),
            label: id.to_string(),
            kind: ts_core::snapshot::WindowKind::Session,
            used_percent: pct,
            resets_at,
            window_minutes: Some(300),
            level: ts_core::snapshot::Level::default(),
            source: source.to_string(),
            observed_at,
        }
    }

    fn config() -> ClaudeConfig {
        ClaudeConfig::default()
    }

    #[test]
    fn decide_proceeds_by_default() {
        assert_eq!(
            decide(&mut LimitsState::default(), &config(), 1000, false),
            Decision::Proceed
        );
    }

    #[test]
    fn decide_records_last_attempt_when_proceeding() {
        let mut st = LimitsState::default();
        decide(&mut st, &config(), 1000, false);
        assert_eq!(st.last_attempt_at, Some(1000));
    }

    #[test]
    fn decide_skips_when_disabled_in_config() {
        let mut c = config();
        c.use_oauth_endpoint = false;
        assert!(matches!(
            decide(&mut LimitsState::default(), &c, 0, false),
            Decision::Skip(_)
        ));
    }

    #[test]
    fn decide_skips_during_429_backoff_even_when_forced() {
        let mut st = LimitsState {
            backoff_until: Some(1000),
            ..Default::default()
        };
        assert!(matches!(
            decide(&mut st, &config(), 500, true),
            Decision::Skip(_)
        ));
    }

    /// `decide` soft-skips at `skip_now`, then proceeds at `proceed_now`.
    fn assert_soft_skip_then_proceed(
        mut st: LimitsState,
        c: &ClaudeConfig,
        skip_now: i64,
        proceed_now: i64,
        force: bool,
    ) {
        assert!(matches!(
            decide(&mut st, c, skip_now, force),
            Decision::SoftSkip(_)
        ));
        assert_eq!(decide(&mut st, c, proceed_now, force), Decision::Proceed);
    }

    #[test]
    fn decide_soft_skips_within_interval_but_proceeds_after() {
        // min_endpoint_interval_secs = 180
        let st = LimitsState {
            last_attempt_at: Some(1000),
            ..Default::default()
        };
        assert_soft_skip_then_proceed(st, &config(), 1010, 1200, false);
    }

    #[test]
    fn decide_force_bypasses_soft_floor_but_not_hard_floor() {
        // force bypasses the 180s soft floor but not the 30s hard floor.
        let st = LimitsState {
            last_attempt_at: Some(1000),
            ..Default::default()
        };
        assert_soft_skip_then_proceed(st, &config(), 1010, 1031, true);
    }

    #[test]
    fn decide_skips_when_endpoint_disabled_by_403() {
        let mut st = LimitsState {
            endpoint_disabled: true,
            disabled_message: Some("no scope".into()),
            ..Default::default()
        };
        match decide(&mut st, &config(), 0, true) {
            Decision::Skip(msg) => assert_eq!(msg, "no scope"),
            _ => panic!("expected skip"),
        }
    }

    #[test]
    fn backoff_clamps_huge_retry_after_instead_of_overflowing() {
        let (until, _) = backoff_after_rate_limit(0, Some(u64::MAX), 180, 1000);
        assert_eq!(until, 1000 + MAX_RETRY_AFTER_SECS);
    }

    #[test]
    fn backoff_uses_retry_after_when_present() {
        let (until, current) = backoff_after_rate_limit(0, Some(45), 180, 1000);
        assert_eq!(until, 1045);
        assert_eq!(current, 0);
    }

    #[test]
    fn backoff_exponential_without_retry_after() {
        let (until1, cur1) = backoff_after_rate_limit(0, None, 180, 0);
        assert_eq!(cur1, 300); // max(180, 300)
        assert_eq!(until1, 300);
        let (until2, cur2) = backoff_after_rate_limit(cur1, None, 180, 300);
        assert_eq!(cur2, 600);
        assert_eq!(until2, 900);
    }

    #[test]
    fn backoff_caps_at_one_hour() {
        let (_, cur) = backoff_after_rate_limit(3000, None, 180, 0);
        assert_eq!(cur, 3600);
        let (_, cur2) = backoff_after_rate_limit(3600, None, 180, 0);
        assert_eq!(cur2, 3600);
    }

    #[test]
    fn next_refresh_never_below_thirty_seconds() {
        let d = next_limits_refresh(
            &LimitsState::default(),
            180,
            1000,
            Duration::from_secs(5),
            std::iter::empty(),
        );
        assert_eq!(d, Duration::from_secs(30));
    }

    #[test]
    fn next_refresh_uses_backoff_when_longer_than_default() {
        let st = LimitsState {
            backoff_until: Some(2000),
            ..Default::default()
        };
        let d = next_limits_refresh(&st, 180, 1000, Duration::from_secs(60), std::iter::empty());
        assert_eq!(d, Duration::from_secs(1000));
    }

    #[test]
    fn next_refresh_uses_earliest_reset_plus_floor() {
        let d = next_limits_refresh(
            &LimitsState::default(),
            180,
            1000,
            Duration::from_secs(3600),
            [1100, 5000].into_iter(),
        );
        assert_eq!(d, Duration::from_secs(130)); // 100 + 30
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "compares exact literals that never went through arithmetic"
    )]
    fn merge_prefers_newer_source_for_session_and_weekly() {
        let endpoint = vec![
            window("session", 10.0, Some(2000), 100, "oauth"),
            window("weekly_all", 20.0, Some(3000), 100, "oauth"),
            window("weekly_scoped:fable", 5.0, Some(3000), 100, "oauth"),
        ];
        let statusline_session = window("session", 15.0, Some(2000), 200, "statusline");
        let merged = merge_windows(&endpoint, Some(&statusline_session), None, 500);
        assert_eq!(merged[0].used_percent, 15.0);
        assert_eq!(merged[0].source, "statusline");
        assert_eq!(merged[1].source, "oauth"); // weekly_all: only endpoint available
        assert_eq!(merged[2].id, "weekly_scoped:fable");
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "compares exact literals that never went through arithmetic"
    )]
    fn merge_applies_rollover_to_past_resets() {
        let endpoint = vec![window("session", 90.0, Some(400), 100, "oauth")];
        let merged = merge_windows(&endpoint, None, None, 500);
        assert_eq!(merged[0].used_percent, 0.0);
        assert_eq!(merged[0].resets_at, None);
        assert_eq!(merged[0].source, "rollover");
    }

    #[test]
    fn state_not_installed_when_nothing_found() {
        let inputs = StateInputs {
            claude_binary_found: false,
            credentials_file_exists: false,
            credentials_expired_with_no_data: false,
            last_outcome: LastOutcome::None,
            has_window_data: false,
            has_fresh_statusline: false,
            projects_dir_exists: false,
        };
        assert_eq!(provider_state(&inputs).0, ProviderState::NotInstalled);
    }

    #[test]
    fn state_unauthenticated_when_no_credentials_but_binary_found() {
        let inputs = StateInputs {
            claude_binary_found: true,
            credentials_file_exists: false,
            credentials_expired_with_no_data: false,
            last_outcome: LastOutcome::None,
            has_window_data: false,
            has_fresh_statusline: false,
            projects_dir_exists: false,
        };
        let (state, msg) = provider_state(&inputs);
        assert_eq!(state, ProviderState::Unauthenticated);
        assert!(msg.unwrap().contains("Not signed in"));
    }

    #[test]
    fn state_ok_when_fresh_statusline_even_if_endpoint_failed() {
        let inputs = StateInputs {
            claude_binary_found: true,
            credentials_file_exists: true,
            credentials_expired_with_no_data: false,
            last_outcome: LastOutcome::Failed("boom".into()),
            has_window_data: true,
            has_fresh_statusline: true,
            projects_dir_exists: true,
        };
        assert_eq!(provider_state(&inputs).0, ProviderState::Ok);
    }

    #[test]
    fn state_stale_when_failed_but_has_data() {
        let inputs = StateInputs {
            claude_binary_found: true,
            credentials_file_exists: true,
            credentials_expired_with_no_data: false,
            last_outcome: LastOutcome::Failed("boom".into()),
            has_window_data: true,
            has_fresh_statusline: false,
            projects_dir_exists: true,
        };
        let (state, msg) = provider_state(&inputs);
        assert_eq!(state, ProviderState::Stale);
        assert_eq!(msg.as_deref(), Some("boom"));
    }

    #[test]
    fn state_error_when_failed_with_no_data() {
        let inputs = StateInputs {
            claude_binary_found: true,
            credentials_file_exists: true,
            credentials_expired_with_no_data: false,
            last_outcome: LastOutcome::Failed("boom".into()),
            has_window_data: false,
            has_fresh_statusline: false,
            projects_dir_exists: true,
        };
        assert_eq!(provider_state(&inputs).0, ProviderState::Error);
    }

    #[test]
    fn state_loading_before_first_attempt() {
        let inputs = StateInputs {
            claude_binary_found: true,
            credentials_file_exists: true,
            credentials_expired_with_no_data: false,
            last_outcome: LastOutcome::None,
            has_window_data: false,
            has_fresh_statusline: false,
            projects_dir_exists: true,
        };
        assert_eq!(provider_state(&inputs).0, ProviderState::Loading);
    }
}
