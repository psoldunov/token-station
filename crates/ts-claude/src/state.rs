//! Pure decision logic: whether to refresh, how long to back off, how to merge
//! endpoint/statusline observations, and which [`ProviderState`] to report.
//!
//! Kept side-effect free so it can be unit tested without a clock or the
//! filesystem; [`crate::provider`] supplies real inputs.

use std::time::Duration;

use ts_core::config::{ClaudeConfig, MIN_CLAUDE_ENDPOINT_INTERVAL_SECS};
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
    /// [`Credentials::plan_fingerprint`] of the sign-in the reading and the
    /// backoff belong to, once one has been seen.
    ///
    /// [`Credentials::plan_fingerprint`]: crate::credentials::Credentials::plan_fingerprint
    pub plan_fingerprint: Option<String>,
}

impl LimitsState {
    /// A clean slate for a sign-in on another plan: nothing the old one was
    /// told carries over, bar the pacing of `claude auth status`.
    #[must_use]
    pub fn for_plan(&self, plan_fingerprint: String) -> LimitsState {
        LimitsState {
            last_auth_status_check: self.last_auth_status_check,
            plan_fingerprint: Some(plan_fingerprint),
            ..LimitsState::default()
        }
    }
}

/// Whether a reading held for plan `held` belongs to a sign-in on another
/// plan than `current`. An unknown plan on either side is not a change.
pub fn plan_changed(held: Option<&str>, current: Option<&str>) -> bool {
    matches!((held, current), (Some(held), Some(current)) if held != current)
}

const HARD_FLOOR_SECS: i64 = 30;
/// First wait after a 429 while a reading is on screen; each consecutive one
/// doubles it, up to the cap. With nothing on screen the first wait is the
/// endpoint floor instead: see [`backoff_after_rate_limit`].
const BACKOFF_BASE_SECS: u64 = 600;
const BACKOFF_CAP_SECS: u64 = 3600;
/// How long a reading stays good enough that a 429 is not worth a warning: the
/// daemon retries on its own, and the numbers on screen are still current.
pub const RATE_LIMIT_GRACE_SECS: i64 = 1800;
pub const RATE_LIMITED_MESSAGE: &str = "Rate limited by Claude's usage endpoint.";
/// A 429 with nothing on screen yet: the daemon is waiting, not failing.
pub const RATE_LIMITED_RETRYING_MESSAGE: &str =
    "Rate limited by Claude's usage endpoint; retrying automatically.";
/// Upper bound on a Claude-supplied `Retry-After` in seconds, so a corrupt or
/// hostile response cannot push `backoff_until` far enough to overflow `i64`
/// arithmetic (or just wedge the provider for an absurd length of time).
pub const MAX_RETRY_AFTER_SECS: i64 = 24 * 3600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Proceed,
    /// A real, persistent reason not to refresh (disabled, ...): worth
    /// recording as the provider's last outcome.
    Skip(String),
    /// The scheduler ticked inside the soft interval floor, or inside a 429
    /// backoff. Not recorded as the last outcome: it says nothing new about
    /// data health (the 429 itself was already recorded) and would flip a
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
            return Decision::SoftSkip(format!(
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
        if now.saturating_sub(last) < floor {
            return Decision::SoftSkip("Refreshed too recently.".to_string());
        }
    }
    state.last_attempt_at = Some(now);
    Decision::Proceed
}

/// Backoff after a 429: returns when to try again and the step to remember.
///
/// Every consecutive 429 doubles the step, whether or not the response carried
/// a `Retry-After`. The header is a lower bound, not a replacement: a short
/// one repeated on every 429 would otherwise pin retries to the base cadence
/// for as long as the endpoint keeps refusing.
///
/// `has_window_data` picks the first step. A reading on screen can afford the
/// ten-minute base; an empty panel cannot, so there the first retry comes at
/// the endpoint floor and the ladder doubles up from it.
pub fn backoff_after_rate_limit(
    prior_step_secs: u64,
    retry_after_secs: Option<u64>,
    min_endpoint_interval_secs: u64,
    has_window_data: bool,
    now: i64,
) -> (i64, u64) {
    let floor = min_endpoint_interval_secs.max(MIN_CLAUDE_ENDPOINT_INTERVAL_SECS);
    let base = if has_window_data {
        floor.max(BACKOFF_BASE_SECS)
    } else {
        floor
    };
    let step = if prior_step_secs == 0 {
        base
    } else {
        // A configured floor above the cap must not make the backoff shrink.
        prior_step_secs
            .saturating_mul(2)
            .min(BACKOFF_CAP_SECS.max(base))
    };
    let step_wait = i64::try_from(step).unwrap_or(i64::MAX);
    let wait = retry_after_secs.map_or(step_wait, |secs| {
        i64::try_from(secs)
            .unwrap_or(i64::MAX)
            .min(MAX_RETRY_AFTER_SECS)
            .max(step_wait)
    });
    (now.saturating_add(wait), step)
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
        state
            .backoff_until
            .map_or(0, |u| u.saturating_sub(now).max(0)),
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
    /// The endpoint answered 429. Kept apart from [`LastOutcome::Failed`]
    /// because the daemon's own backoff deals with it: it only warrants a
    /// warning once the reading on screen is no longer recent.
    RateLimited,
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
    /// The endpoint's last good reading is younger than [`RATE_LIMIT_GRACE_SECS`].
    pub has_recent_endpoint_data: bool,
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
        LastOutcome::Failed(msg) => failed(inputs, msg),
        LastOutcome::RateLimited if inputs.has_recent_endpoint_data => (ProviderState::Ok, None),
        LastOutcome::RateLimited if inputs.has_window_data => {
            (ProviderState::Stale, Some(RATE_LIMITED_MESSAGE.to_string()))
        }
        LastOutcome::RateLimited => (
            ProviderState::Loading,
            Some(RATE_LIMITED_RETRYING_MESSAGE.to_string()),
        ),
    }
}

/// The last outcome to assume for a reading carried over from an earlier run.
///
/// A backoff still running means the endpoint was refusing; a reading inside
/// the rate-limit grace is as good as a fresh one; anything older is left for
/// the next refresh to judge, which shows it as stale in the meantime.
pub fn restored_outcome(
    backoff_until: Option<i64>,
    last_endpoint_success_at: Option<i64>,
    now: i64,
) -> LastOutcome {
    if backoff_until.is_some_and(|until| until > now) {
        LastOutcome::RateLimited
    } else if last_endpoint_success_at
        .is_some_and(|t| now.saturating_sub(t) < RATE_LIMIT_GRACE_SECS)
    {
        LastOutcome::Ok
    } else {
        LastOutcome::None
    }
}

/// A failure shown over whatever data there is: stale data, or an error with none.
fn failed(inputs: &StateInputs, msg: &str) -> (ProviderState, Option<String>) {
    let state = if inputs.has_window_data {
        ProviderState::Stale
    } else {
        ProviderState::Error
    };
    (state, Some(msg.to_string()))
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
    fn decide_soft_skips_during_429_backoff_even_when_forced() {
        // Soft, so the 429 already recorded stays the last outcome.
        let mut st = LimitsState {
            backoff_until: Some(1000),
            ..Default::default()
        };
        assert!(matches!(
            decide(&mut st, &config(), 500, true),
            Decision::SoftSkip(_)
        ));
        assert_eq!(decide(&mut st, &config(), 1000, true), Decision::Proceed);
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
        // min_endpoint_interval_secs = 300
        let st = LimitsState {
            last_attempt_at: Some(1000),
            ..Default::default()
        };
        assert_soft_skip_then_proceed(st, &config(), 1299, 1300, false);
    }

    #[test]
    fn decide_force_bypasses_soft_floor_but_not_hard_floor() {
        // force bypasses the 300s soft floor but not the 30s hard floor.
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
        let (until, _) = backoff_after_rate_limit(0, Some(u64::MAX), 180, true, 1000);
        assert_eq!(until, 1000 + MAX_RETRY_AFTER_SECS);
    }

    #[test]
    fn backoff_waits_for_the_longer_of_retry_after_and_the_step() {
        // (Retry-After, expected wait): the 600 s first step is the floor.
        for (retry_after, wait) in [(900, 900), (45, 600)] {
            let (until, step) = backoff_after_rate_limit(0, Some(retry_after), 300, true, 1000);
            assert_eq!(
                (until, step),
                (1000 + wait, 600),
                "Retry-After {retry_after}"
            );
        }
    }

    #[test]
    fn backoff_escalates_on_repeated_429s_with_retry_after() {
        let (_, step1) = backoff_after_rate_limit(0, Some(60), 300, true, 0);
        let (until2, step2) = backoff_after_rate_limit(step1, Some(60), 300, true, 600);
        assert_eq!(step2, 1200);
        assert_eq!(until2, 1800);
    }

    #[test]
    fn backoff_exponential_without_retry_after() {
        let (until1, cur1) = backoff_after_rate_limit(0, None, 300, true, 0);
        assert_eq!(cur1, 600); // max(300, 600)
        assert_eq!(until1, 600);
        let (until2, cur2) = backoff_after_rate_limit(cur1, None, 300, true, 600);
        assert_eq!(cur2, 1200);
        assert_eq!(until2, 1800);
    }

    #[test]
    fn backoff_starts_at_a_configured_floor_above_the_base() {
        let (until, step) = backoff_after_rate_limit(0, None, 900, true, 0);
        assert_eq!((until, step), (900, 900));
    }

    #[test]
    fn backoff_caps_at_one_hour() {
        let (_, cur) = backoff_after_rate_limit(3000, None, 300, true, 0);
        assert_eq!(cur, 3600);
        let (_, cur2) = backoff_after_rate_limit(3600, None, 300, true, 0);
        assert_eq!(cur2, 3600);
    }

    #[test]
    fn backoff_never_shrinks_below_a_configured_floor_above_the_cap() {
        let (_, step) = backoff_after_rate_limit(7200, None, 7200, true, 0);
        assert_eq!(step, 7200);
    }

    #[test]
    fn backoff_starts_at_the_endpoint_floor_with_nothing_on_screen() {
        // Nothing to show: the first retry comes at the configured floor, not
        // at the ten minutes a reading on screen can afford to wait.
        let (until, step) = backoff_after_rate_limit(0, None, 300, false, 1000);
        assert_eq!((until, step), (1300, 300));
    }

    #[test]
    fn cold_backoff_never_starts_below_the_lowest_allowed_floor() {
        let (until, step) = backoff_after_rate_limit(0, None, 30, false, 0);
        assert_eq!((until, step), (120, 120));
    }

    #[test]
    fn cold_backoff_still_waits_for_a_longer_retry_after() {
        let (until, step) = backoff_after_rate_limit(0, Some(900), 300, false, 0);
        assert_eq!((until, step), (900, 300));
    }

    #[test]
    fn cold_backoff_escalates_onto_the_same_ladder() {
        let (_, first) = backoff_after_rate_limit(0, None, 300, false, 0);
        let (until, second) = backoff_after_rate_limit(first, None, 300, false, 300);
        assert_eq!((until, second), (900, 600));
    }

    #[test]
    fn a_restored_backoff_still_running_means_rate_limited() {
        assert_eq!(
            restored_outcome(Some(2000), Some(1000), 1500),
            LastOutcome::RateLimited
        );
    }

    #[test]
    fn a_restored_recent_reading_counts_as_ok() {
        assert_eq!(
            restored_outcome(Some(1400), Some(1000), 1500),
            LastOutcome::Ok
        );
    }

    #[test]
    fn a_restored_old_reading_is_left_for_the_next_refresh_to_judge() {
        let now = 1000 + RATE_LIMIT_GRACE_SECS;
        assert_eq!(restored_outcome(None, Some(1000), now), LastOutcome::None);
        assert_eq!(restored_outcome(None, None, now), LastOutcome::None);
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
            has_recent_endpoint_data: false,
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
            has_recent_endpoint_data: false,
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
            has_recent_endpoint_data: false,
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
            has_recent_endpoint_data: false,
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
            has_recent_endpoint_data: false,
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
            has_recent_endpoint_data: false,
            projects_dir_exists: true,
        };
        assert_eq!(provider_state(&inputs).0, ProviderState::Loading);
    }

    /// Signed in, with a 429 as the last outcome.
    fn rate_limited(has_window_data: bool, has_recent_endpoint_data: bool) -> StateInputs {
        StateInputs {
            claude_binary_found: true,
            credentials_file_exists: true,
            credentials_expired_with_no_data: false,
            last_outcome: LastOutcome::RateLimited,
            has_window_data,
            has_fresh_statusline: false,
            has_recent_endpoint_data,
            projects_dir_exists: true,
        }
    }

    #[test]
    fn state_ok_without_a_warning_when_rate_limited_over_a_recent_reading() {
        assert_eq!(
            provider_state(&rate_limited(true, true)),
            (ProviderState::Ok, None)
        );
    }

    #[test]
    fn state_stale_with_a_warning_when_rate_limited_over_an_old_reading() {
        assert_eq!(
            provider_state(&rate_limited(true, false)),
            (ProviderState::Stale, Some(RATE_LIMITED_MESSAGE.to_string()))
        );
    }

    #[test]
    fn state_loading_while_rate_limited_with_no_data() {
        // Nothing has gone wrong that the daemon will not retry on its own, so
        // an empty panel reads as waiting rather than as an error.
        assert_eq!(
            provider_state(&rate_limited(false, false)),
            (
                ProviderState::Loading,
                Some(RATE_LIMITED_RETRYING_MESSAGE.to_string())
            )
        );
    }
}
