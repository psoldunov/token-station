//! What the provider keeps across a daemon restart: the last plan-limit
//! reading and the rate-limit backoff.
//!
//! Without it every restart starts empty, asks the endpoint straight away
//! whatever the backoff said, and has nothing to show when that first request
//! is refused. The daemon owns the file; this module only decides what goes
//! into it and how much of what comes back out is still believable.

use serde::{Deserialize, Serialize};
use ts_core::snapshot::{BreakdownRow, Credits, UsageWindow};

use crate::provider::Observations;
use crate::state::{self, LastOutcome, LimitsState, MAX_RETRY_AFTER_SECS};

/// Bumped whenever a field changes meaning; any other version is dropped.
pub const SAVED_VERSION: u32 = 1;

/// A reading older than this describes a week that is already over.
pub const MAX_SAVED_AGE_SECS: i64 = 7 * 86_400;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedLimits {
    pub version: u32,
    #[serde(default)]
    pub endpoint_windows: Vec<UsageWindow>,
    #[serde(default)]
    pub statusline_session: Option<UsageWindow>,
    #[serde(default)]
    pub statusline_weekly_all: Option<UsageWindow>,
    #[serde(default)]
    pub credits: Option<Credits>,
    #[serde(default)]
    pub breakdown: Vec<BreakdownRow>,
    #[serde(default)]
    pub plan: Option<String>,
    #[serde(default)]
    pub last_endpoint_success_at: Option<i64>,
    #[serde(default)]
    pub last_attempt_at: Option<i64>,
    #[serde(default)]
    pub backoff_until: Option<i64>,
    #[serde(default)]
    pub backoff_current_secs: u64,
    /// The plan of the sign-in all of the above belongs to.
    #[serde(default)]
    pub plan_fingerprint: Option<String>,
}

impl SavedLimits {
    /// What `limits` and `obs` hold that is worth keeping.
    pub fn capture(limits: &LimitsState, obs: &Observations) -> SavedLimits {
        SavedLimits {
            version: SAVED_VERSION,
            endpoint_windows: obs.endpoint_windows.clone(),
            statusline_session: obs.statusline_session.clone(),
            statusline_weekly_all: obs.statusline_weekly_all.clone(),
            credits: obs.credits.clone(),
            breakdown: obs.breakdown.clone(),
            plan: obs.plan.clone(),
            last_endpoint_success_at: obs.last_endpoint_success_at,
            last_attempt_at: limits.last_attempt_at,
            backoff_until: limits.backoff_until,
            backoff_current_secs: limits.backoff_current_secs,
            plan_fingerprint: limits.plan_fingerprint.clone(),
        }
    }

    /// Fold a restored value into `limits` and `obs`.
    ///
    /// Only where it is newer than what is already there: the bus is up
    /// before the saved file is read, so a statusline may have arrived first,
    /// and a restore must never roll that back. Nothing at all is taken from
    /// a value saved for a sign-in on another plan than the one held.
    pub fn apply_to(self, limits: &mut LimitsState, obs: &mut Observations, now: i64) {
        if state::plan_changed(
            limits.plan_fingerprint.as_deref(),
            self.plan_fingerprint.as_deref(),
        ) {
            tracing::info!("ignoring saved Claude limits from a sign-in on another plan");
            return;
        }
        if limits.plan_fingerprint.is_none() {
            limits.plan_fingerprint = self.plan_fingerprint;
        }
        if obs.last_endpoint_success_at < self.last_endpoint_success_at {
            obs.endpoint_windows = self.endpoint_windows;
            obs.credits = self.credits;
            obs.breakdown = self.breakdown;
            obs.plan = self.plan;
            obs.last_endpoint_success_at = self.last_endpoint_success_at;
        }
        obs.statusline_session = newer(obs.statusline_session.take(), self.statusline_session);
        obs.statusline_weekly_all =
            newer(obs.statusline_weekly_all.take(), self.statusline_weekly_all);
        // An attempt made on this run has already had the endpoint's newer
        // verdict on the backoff — a success clears it to `None` — and the
        // file must not bring the old one back.
        if limits.last_attempt_at.is_none() && limits.backoff_until < self.backoff_until {
            limits.backoff_until = self.backoff_until;
            limits.backoff_current_secs = self.backoff_current_secs;
        }
        limits.last_attempt_at = limits.last_attempt_at.max(self.last_attempt_at);
        if limits.last_outcome == LastOutcome::None {
            limits.last_outcome =
                state::restored_outcome(limits.backoff_until, obs.last_endpoint_success_at, now);
        }
    }

    /// Whether there is anything here worth writing down.
    pub fn is_empty(&self) -> bool {
        self.last_attempt_at.is_none()
            && self.last_endpoint_success_at.is_none()
            && self.backoff_until.is_none()
            && self.statusline_session.is_none()
            && self.statusline_weekly_all.is_none()
    }

    /// Read a saved value back, keeping only what is still believable at `now`.
    ///
    /// `None` for anything unreadable or from another version. Otherwise
    /// every timestamp is pulled back to `now` at the latest — a clock set
    /// back since the save would leave a future attempt holding off refreshes
    /// until it caught up — a backoff is cut to the longest `Retry-After` the
    /// provider would honour, and a backoff already over, or any reading or
    /// attempt older than a week, is dropped. Every timestamp is bounded on
    /// both sides that way, so no arithmetic on one can overflow.
    pub fn restore(value: serde_json::Value, now: i64) -> Option<SavedLimits> {
        let saved: SavedLimits = match serde_json::from_value(value) {
            Ok(saved) => saved,
            Err(error) => {
                tracing::warn!(%error, "ignoring unreadable saved Claude limits");
                return None;
            }
        };
        if saved.version != SAVED_VERSION {
            tracing::warn!(
                version = saved.version,
                "ignoring saved Claude limits from another version"
            );
            return None;
        }
        Some(saved.believable_at(now))
    }

    fn believable_at(self, now: i64) -> SavedLimits {
        let last_endpoint_success_at = self.last_endpoint_success_at.map(|t| t.min(now));
        let reading_is_current = last_endpoint_success_at.is_some_and(|t| is_recent(t, now));
        let reading = if reading_is_current {
            EndpointReading {
                windows: self
                    .endpoint_windows
                    .into_iter()
                    .map(|w| clamp(w, now))
                    .collect(),
                credits: self.credits,
                breakdown: self.breakdown,
                plan: self.plan,
                at: last_endpoint_success_at,
            }
        } else {
            EndpointReading::default()
        };
        SavedLimits {
            version: SAVED_VERSION,
            endpoint_windows: reading.windows,
            statusline_session: current_window(self.statusline_session, now),
            statusline_weekly_all: current_window(self.statusline_weekly_all, now),
            credits: reading.credits,
            breakdown: reading.breakdown,
            plan: reading.plan,
            last_endpoint_success_at: reading.at,
            last_attempt_at: self
                .last_attempt_at
                .map(|t| t.min(now))
                .filter(|&t| is_recent(t, now)),
            backoff_until: self
                .backoff_until
                .filter(|&until| until > now)
                .map(|until| until.min(now.saturating_add(MAX_RETRY_AFTER_SECS))),
            backoff_current_secs: self.backoff_current_secs,
            plan_fingerprint: self.plan_fingerprint,
        }
    }
}

/// Whether `t`, at or before `now`, is recent enough to still describe it.
fn is_recent(t: i64, now: i64) -> bool {
    now.saturating_sub(t) <= MAX_SAVED_AGE_SECS
}

/// The endpoint's half of a saved reading, kept or dropped as one.
#[derive(Default)]
struct EndpointReading {
    windows: Vec<UsageWindow>,
    credits: Option<Credits>,
    breakdown: Vec<BreakdownRow>,
    plan: Option<String>,
    at: Option<i64>,
}

fn clamp(window: UsageWindow, now: i64) -> UsageWindow {
    UsageWindow {
        observed_at: window.observed_at.min(now),
        ..window
    }
}

fn newer(held: Option<UsageWindow>, saved: Option<UsageWindow>) -> Option<UsageWindow> {
    match (held, saved) {
        (Some(held), Some(saved)) if saved.observed_at > held.observed_at => Some(saved),
        (held, saved) => held.or(saved),
    }
}

fn current_window(window: Option<UsageWindow>, now: i64) -> Option<UsageWindow> {
    window
        .map(|w| clamp(w, now))
        .filter(|w| is_recent(w.observed_at, now))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts_core::snapshot::{Level, WindowKind};

    const NOW: i64 = 1_790_596_800;

    fn window(id: &str, observed_at: i64) -> UsageWindow {
        UsageWindow {
            id: id.to_string(),
            label: id.to_string(),
            kind: WindowKind::Session,
            used_percent: 40.0,
            resets_at: Some(NOW + 3600),
            window_minutes: Some(300),
            level: Level::Normal,
            source: "statusline".to_string(),
            observed_at,
        }
    }

    fn restore(saved: &SavedLimits, now: i64) -> SavedLimits {
        SavedLimits::restore(serde_json::to_value(saved).unwrap(), now).unwrap()
    }

    #[test]
    fn a_saved_reading_round_trips_unchanged() {
        let saved = SavedLimits {
            version: SAVED_VERSION,
            endpoint_windows: vec![window("session", NOW - 60)],
            statusline_session: Some(window("session", NOW - 30)),
            plan: Some("Max 20x".to_string()),
            last_endpoint_success_at: Some(NOW - 60),
            last_attempt_at: Some(NOW - 60),
            backoff_until: Some(NOW + 600),
            backoff_current_secs: 600,
            ..SavedLimits::default()
        };
        assert_eq!(restore(&saved, NOW), saved);
    }

    #[test]
    fn a_window_observed_in_the_future_is_pulled_back_to_now() {
        let saved = SavedLimits {
            version: SAVED_VERSION,
            statusline_session: Some(window("session", NOW + 5000)),
            ..SavedLimits::default()
        };
        let restored = restore(&saved, NOW);
        assert_eq!(restored.statusline_session.unwrap().observed_at, NOW);
    }

    #[test]
    fn a_statusline_window_older_than_a_week_is_dropped() {
        let saved = SavedLimits {
            version: SAVED_VERSION,
            statusline_weekly_all: Some(window("weekly_all", NOW - MAX_SAVED_AGE_SECS - 1)),
            ..SavedLimits::default()
        };
        assert_eq!(restore(&saved, NOW).statusline_weekly_all, None);
    }

    #[test]
    fn a_backoff_is_kept_even_when_the_reading_is_dropped() {
        let saved = SavedLimits {
            version: SAVED_VERSION,
            endpoint_windows: vec![window("session", NOW - MAX_SAVED_AGE_SECS - 1)],
            last_endpoint_success_at: Some(NOW - MAX_SAVED_AGE_SECS - 1),
            backoff_until: Some(NOW + 600),
            backoff_current_secs: 1200,
            ..SavedLimits::default()
        };
        let restored = restore(&saved, NOW);
        assert!(restored.endpoint_windows.is_empty());
        assert_eq!(restored.last_endpoint_success_at, None);
        assert_eq!(
            (restored.backoff_until, restored.backoff_current_secs),
            (Some(NOW + 600), 1200)
        );
    }

    #[test]
    fn a_backoff_already_over_is_dropped() {
        let saved = SavedLimits {
            version: SAVED_VERSION,
            backoff_until: Some(NOW - 1),
            backoff_current_secs: 1200,
            ..SavedLimits::default()
        };
        let restored = restore(&saved, NOW);
        assert_eq!(restored.backoff_until, None);
        // The step stays, so the next 429 still climbs the ladder.
        assert_eq!(restored.backoff_current_secs, 1200);
    }

    #[test]
    fn extreme_timestamps_are_dropped_or_bounded() {
        for t in [i64::MIN, i64::MIN + 1, -1, 0, i64::MAX] {
            let saved = SavedLimits {
                version: SAVED_VERSION,
                endpoint_windows: vec![window("session", t)],
                statusline_session: Some(window("session", t)),
                last_endpoint_success_at: Some(t),
                last_attempt_at: Some(t),
                backoff_until: Some(t),
                ..SavedLimits::default()
            };
            let restored = restore(&saved, NOW);
            for at in [restored.last_endpoint_success_at, restored.last_attempt_at]
                .into_iter()
                .flatten()
                .chain(restored.statusline_session.as_ref().map(|w| w.observed_at))
            {
                assert!((NOW - MAX_SAVED_AGE_SECS..=NOW).contains(&at), "{t}: {at}");
            }
            if let Some(until) = restored.backoff_until {
                assert!(until > NOW && until <= NOW + MAX_RETRY_AFTER_SECS, "{t}");
            }

            // And what is left is safe to decide on and schedule from.
            let (mut limits, mut obs) = (LimitsState::default(), Observations::default());
            restored.apply_to(&mut limits, &mut obs, NOW);
            let config = ts_core::config::ClaudeConfig::default();
            let _ = state::decide(&mut limits.clone(), &config, NOW, false);
            let _ = state::next_limits_refresh(
                &limits,
                config.min_endpoint_interval_secs,
                NOW,
                std::time::Duration::from_secs(60),
                std::iter::empty(),
            );
        }
    }

    fn held_limits() -> LimitsState {
        LimitsState {
            plan_fingerprint: Some("max/tier".to_string()),
            ..LimitsState::default()
        }
    }

    fn saved_limits() -> SavedLimits {
        SavedLimits {
            version: SAVED_VERSION,
            endpoint_windows: vec![window("session", NOW - 600)],
            statusline_session: Some(window("session", NOW - 600)),
            plan: Some("Max 20x".to_string()),
            last_endpoint_success_at: Some(NOW - 600),
            last_attempt_at: Some(NOW - 600),
            backoff_until: Some(NOW + 600),
            backoff_current_secs: 600,
            plan_fingerprint: Some("max/tier".to_string()),
            ..SavedLimits::default()
        }
    }

    #[test]
    fn a_restore_into_an_empty_provider_takes_everything() {
        let (mut limits, mut obs) = (held_limits(), Observations::default());
        saved_limits().apply_to(&mut limits, &mut obs, NOW);
        assert_eq!(obs.last_endpoint_success_at, Some(NOW - 600));
        assert_eq!(obs.plan.as_deref(), Some("Max 20x"));
        assert_eq!(
            obs.statusline_session.map(|w| w.observed_at),
            Some(NOW - 600)
        );
        assert_eq!(limits.last_attempt_at, Some(NOW - 600));
        assert_eq!(
            (limits.backoff_until, limits.backoff_current_secs),
            (Some(NOW + 600), 600)
        );
        assert_eq!(limits.last_outcome, LastOutcome::RateLimited);
    }

    #[test]
    fn a_restore_never_rolls_back_what_this_run_has_seen() {
        let mut limits = held_limits();
        let mut obs = Observations {
            endpoint_windows: vec![window("session", NOW - 5)],
            statusline_session: Some(window("session", NOW - 5)),
            plan: Some("Pro".to_string()),
            last_endpoint_success_at: Some(NOW - 5),
            ..Observations::default()
        };
        saved_limits().apply_to(&mut limits, &mut obs, NOW);
        assert_eq!(obs.last_endpoint_success_at, Some(NOW - 5));
        assert_eq!(obs.plan.as_deref(), Some("Pro"));
        assert_eq!(obs.endpoint_windows[0].observed_at, NOW - 5);
        assert_eq!(obs.statusline_session.map(|w| w.observed_at), Some(NOW - 5));
    }

    #[test]
    fn a_newer_saved_statusline_replaces_an_older_one() {
        let (mut limits, mut obs) = (
            held_limits(),
            Observations {
                statusline_session: Some(window("session", NOW - 900)),
                ..Observations::default()
            },
        );
        saved_limits().apply_to(&mut limits, &mut obs, NOW);
        assert_eq!(
            obs.statusline_session.map(|w| w.observed_at),
            Some(NOW - 600)
        );
    }

    #[test]
    fn a_backoff_this_run_has_cleared_is_not_brought_back() {
        // This run asked and got an answer, which cleared the backoff.
        let mut limits = LimitsState {
            last_attempt_at: Some(NOW - 10),
            last_outcome: LastOutcome::Ok,
            ..held_limits()
        };
        let mut obs = Observations::default();
        saved_limits().apply_to(&mut limits, &mut obs, NOW);
        assert_eq!(
            (limits.backoff_until, limits.backoff_current_secs),
            (None, 0)
        );
        assert_eq!(limits.last_attempt_at, Some(NOW - 10));
        assert_eq!(limits.last_outcome, LastOutcome::Ok);
    }

    #[test]
    fn a_restore_from_another_plan_is_ignored() {
        let (mut limits, mut obs) = (
            LimitsState {
                plan_fingerprint: Some("pro/other".to_string()),
                ..LimitsState::default()
            },
            Observations::default(),
        );
        saved_limits().apply_to(&mut limits, &mut obs, NOW);
        assert_eq!(limits.backoff_until, None);
        assert_eq!(limits.last_attempt_at, None);
        assert_eq!(limits.last_outcome, LastOutcome::None);
        assert_eq!(obs.last_endpoint_success_at, None);
        assert_eq!(obs.statusline_session, None);
        assert_eq!(limits.plan_fingerprint.as_deref(), Some("pro/other"));
    }

    #[test]
    fn a_restore_before_any_sign_in_was_read_adopts_the_saved_plan() {
        let (mut limits, mut obs) = (LimitsState::default(), Observations::default());
        saved_limits().apply_to(&mut limits, &mut obs, NOW);
        assert_eq!(limits.plan_fingerprint.as_deref(), Some("max/tier"));
        assert_eq!(limits.backoff_until, Some(NOW + 600));
    }

    #[test]
    fn nothing_attempted_and_nothing_seen_is_empty() {
        assert!(SavedLimits::default().is_empty());
        let backing_off = SavedLimits {
            backoff_until: Some(NOW),
            ..SavedLimits::default()
        };
        assert!(!backing_off.is_empty());
    }
}
