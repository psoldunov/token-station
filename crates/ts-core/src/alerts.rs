//! Threshold notification decisions.
//!
//! Pure logic: given the persisted [`AlertState`] and the latest snapshot, decide
//! which notifications to send and return the next state. Each threshold fires at
//! most once per window per reset period, so restarts and repeated polls never spam.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::config::AlertsConfig;
use crate::snapshot::{Level, ProviderId, ProviderSnapshot, ProviderState};

/// Two `resets_at` values closer than this belong to the same period (backends
/// jitter reset timestamps by fractions of a second between polls).
pub const PERIOD_TOLERANCE_SECS: i64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertKind {
    Warning,
    Critical,
    Reset,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alert {
    pub provider: ProviderId,
    pub window_id: String,
    pub label: String,
    pub kind: AlertKind,
    pub percent: f64,
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlertEntry {
    /// `resets_at` of the period this entry describes.
    pub period: Option<i64>,
    /// Highest level already notified in this period.
    pub notified: Level,
}

/// Persisted between daemon runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AlertState {
    pub entries: BTreeMap<String, AlertEntry>,
}

fn key(provider: ProviderId, window_id: &str) -> String {
    format!("{provider}/{window_id}")
}

fn same_period(a: Option<i64>, b: Option<i64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() < PERIOD_TOLERANCE_SECS,
        (None, None) => true,
        _ => false,
    }
}

/// Decide notifications for `providers` and return the next state.
///
/// Providers whose state is not `ok` are skipped and keep their entries.
#[must_use]
pub fn evaluate(
    state: &AlertState,
    providers: &[ProviderSnapshot],
    cfg: &AlertsConfig,
) -> (Vec<Alert>, AlertState) {
    let mut alerts = Vec::new();
    let mut next = state.entries.clone();

    for provider in providers.iter().filter(|p| p.state == ProviderState::Ok) {
        for window in &provider.windows {
            let k = key(provider.id, &window.id);
            let level = Level::from_percent(
                window.used_percent,
                cfg.warning_percent,
                cfg.critical_percent,
            );
            let previous = state.entries.get(&k);
            let new_period = previous.is_some_and(|e| !same_period(e.period, window.resets_at));
            let already = match previous {
                Some(e) if !new_period => e.notified,
                _ => Level::Normal,
            };

            let make = |kind| Alert {
                provider: provider.id,
                window_id: window.id.clone(),
                label: window.label.clone(),
                kind,
                percent: window.used_percent,
                resets_at: window.resets_at,
            };

            if new_period
                && cfg.notify_on_reset
                && previous.is_some_and(|e| e.notified > Level::Normal)
            {
                alerts.push(make(AlertKind::Reset));
            }
            if cfg.notify && level > already {
                alerts.push(make(match level {
                    Level::Critical => AlertKind::Critical,
                    _ => AlertKind::Warning,
                }));
            }
            next.insert(
                k,
                AlertEntry {
                    period: window.resets_at,
                    notified: level.max(already),
                },
            );
        }
    }

    (alerts, AlertState { entries: next })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{UsageWindow, WindowKind};

    fn provider(pct: f64, resets_at: i64) -> ProviderSnapshot {
        ProviderSnapshot {
            windows: vec![UsageWindow {
                id: "session".into(),
                label: "Session".into(),
                kind: WindowKind::Session,
                used_percent: pct,
                resets_at: Some(resets_at),
                window_minutes: Some(300),
                level: Level::Normal,
                source: "oauth".into(),
                observed_at: 0,
            }],
            ..ProviderSnapshot::empty(ProviderId::Claude, ProviderState::Ok)
        }
    }

    fn cfg() -> AlertsConfig {
        AlertsConfig {
            notify_on_reset: true,
            ..AlertsConfig::default()
        }
    }

    #[test]
    fn fires_each_threshold_once_per_period() {
        let s0 = AlertState::default();
        let (a1, s1) = evaluate(&s0, &[provider(50.0, 10_000)], &cfg());
        assert!(a1.is_empty());
        let (a2, s2) = evaluate(&s1, &[provider(82.0, 10_000)], &cfg());
        assert_eq!(a2.len(), 1);
        assert_eq!(a2[0].kind, AlertKind::Warning);
        let (a3, s3) = evaluate(&s2, &[provider(85.0, 10_010)], &cfg());
        assert!(
            a3.is_empty(),
            "same period (jittered resets_at) must not repeat"
        );
        let (a4, s4) = evaluate(&s3, &[provider(97.0, 10_000)], &cfg());
        assert_eq!(a4[0].kind, AlertKind::Critical);
        let (a5, _) = evaluate(&s4, &[provider(98.0, 10_000)], &cfg());
        assert!(a5.is_empty());
    }

    #[test]
    fn jumping_straight_to_critical_sends_one_alert() {
        let (a, s) = evaluate(&AlertState::default(), &[provider(99.0, 1)], &cfg());
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].kind, AlertKind::Critical);
        assert_eq!(s.entries["claude/session"].notified, Level::Critical);
    }

    #[test]
    fn new_period_rearms_and_optionally_notifies_reset() {
        let (_, s1) = evaluate(&AlertState::default(), &[provider(90.0, 10_000)], &cfg());
        let (a2, s2) = evaluate(&s1, &[provider(2.0, 30_000)], &cfg());
        assert_eq!(a2.len(), 1);
        assert_eq!(a2[0].kind, AlertKind::Reset);
        assert_eq!(s2.entries["claude/session"].notified, Level::Normal);
        let (a3, _) = evaluate(&s2, &[provider(81.0, 30_000)], &cfg());
        assert_eq!(a3[0].kind, AlertKind::Warning);
    }

    #[test]
    fn reset_notice_respects_config_and_quiet_periods() {
        let quiet = AlertsConfig::default();
        let (_, s1) = evaluate(&AlertState::default(), &[provider(90.0, 10_000)], &quiet);
        let (a, _) = evaluate(&s1, &[provider(1.0, 30_000)], &quiet);
        assert!(a.is_empty());
        // A period that never crossed a threshold does not announce its reset.
        let (_, s2) = evaluate(&AlertState::default(), &[provider(10.0, 10_000)], &cfg());
        let (a2, _) = evaluate(&s2, &[provider(1.0, 30_000)], &cfg());
        assert!(a2.is_empty());
    }

    #[test]
    fn skips_non_ok_providers_and_respects_notify_flag() {
        let mut stale = provider(99.0, 1);
        stale.state = ProviderState::Stale;
        let (a, s) = evaluate(&AlertState::default(), &[stale], &cfg());
        assert!(a.is_empty());
        assert!(s.entries.is_empty());

        let off = AlertsConfig {
            notify: false,
            ..AlertsConfig::default()
        };
        let (a2, s2) = evaluate(&AlertState::default(), &[provider(99.0, 1)], &off);
        assert!(a2.is_empty());
        assert_eq!(s2.entries["claude/session"].notified, Level::Critical);
    }

    #[test]
    fn state_round_trips_as_json() {
        let (_, s) = evaluate(&AlertState::default(), &[provider(85.0, 5)], &cfg());
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<AlertState>(&json).unwrap(), s);
    }
}
