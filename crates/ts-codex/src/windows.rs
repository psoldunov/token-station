//! Pure mapping from app-server DTOs to the snapshot's plan/window/credit shapes.

use ts_core::snapshot::{Credits, UsageWindow, WindowKind};

use crate::dto::{
    CreditsDto, RateLimitResetCreditsDto, RateLimitSnapshotDto, RateLimitsReadResult,
};

/// Human label for a plan type, e.g. `"pro"` -> `"Pro"`.
pub fn plan_label(plan_type: &str) -> String {
    match plan_type {
        "pro" => "Pro".to_string(),
        "plus" => "Plus".to_string(),
        "prolite" => "Pro Lite".to_string(),
        "free" => "Free".to_string(),
        "go" => "Go".to_string(),
        "team" => "Team".to_string(),
        "business" => "Business".to_string(),
        "enterprise" => "Enterprise".to_string(),
        "edu" => "Edu".to_string(),
        other => title_case(other),
    }
}

fn title_case(s: &str) -> String {
    s.split(['_', '-'])
        .filter(|w| !w.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Base label derived only from the window's duration, e.g. `"Session"`, `"Weekly"`.
fn base_label(window_minutes: Option<i64>) -> String {
    match window_minutes {
        Some(300) => "Session".to_string(),
        Some(10080) => "Weekly".to_string(),
        Some(mins) if mins < 1440 => {
            let hours = (mins as f64 / 60.0).round() as i64;
            format!("{hours}-hour window")
        }
        Some(mins) => {
            let days = (mins as f64 / 1440.0).round() as i64;
            format!("{days}-day window")
        }
        None => "Window".to_string(),
    }
}

/// The full window label, prefixed with the limit's model/name for non-`codex` limits.
pub fn window_label(
    limit_id: &str,
    limit_name: Option<&str>,
    normal_model_slug: Option<&str>,
    window_minutes: Option<i64>,
) -> String {
    let base = base_label(window_minutes);
    if limit_id == "codex" {
        return base;
    }
    let suffix = limit_name
        .filter(|s| !s.is_empty())
        .or(normal_model_slug.filter(|s| !s.is_empty()))
        .unwrap_or(limit_id);
    format!("{base} · {suffix}")
}

/// What kind of window this is, for front-end grouping.
pub fn window_kind(limit_id: &str, window_minutes: Option<i64>) -> WindowKind {
    match window_minutes {
        Some(mins) if mins <= 1440 => WindowKind::Session,
        Some(10080) if limit_id == "codex" => WindowKind::Weekly,
        Some(10080) => WindowKind::Model,
        _ => WindowKind::Other,
    }
}

/// One [`UsageWindow`] per non-null primary/secondary window in `snapshot`.
///
/// `limit_id` is the caller-resolved id (the snapshot's own `limitId` field is
/// frequently null; callers fall back to the `rateLimitsByLimitId` map key).
pub fn map_rate_limit_snapshot(
    limit_id: &str,
    snapshot: &RateLimitSnapshotDto,
    observed_at: i64,
    source: &str,
) -> Vec<UsageWindow> {
    let mut windows = Vec::new();
    for (suffix, window) in [
        ("primary", snapshot.primary.as_ref()),
        ("secondary", snapshot.secondary.as_ref()),
    ] {
        if let Some(w) = window {
            windows.push(UsageWindow {
                id: format!("{limit_id}:{suffix}"),
                label: window_label(
                    limit_id,
                    snapshot.limit_name.as_deref(),
                    snapshot.normal_model_slug.as_deref(),
                    w.window_duration_mins,
                ),
                kind: window_kind(limit_id, w.window_duration_mins),
                used_percent: w.used_percent,
                resets_at: w.resets_at,
                window_minutes: w.window_duration_mins.and_then(|m| u32::try_from(m).ok()),
                level: ts_core::snapshot::Level::default(),
                source: source.to_string(),
                observed_at,
            });
        }
    }
    windows
}

/// Windows, credits and plan label for a full `rateLimits/read` result.
///
/// Prefers `rateLimitsByLimitId`; falls back to the single `rateLimits` field
/// when the map is empty (older servers).
pub fn map_rate_limits(
    result: &RateLimitsReadResult,
    observed_at: i64,
) -> (Vec<UsageWindow>, Option<Credits>, Option<String>) {
    let mut windows = Vec::new();
    let mut credits = None;
    let mut plan = None;

    let mut entries: Vec<(&str, &RateLimitSnapshotDto)> = result
        .rate_limits_by_limit_id
        .iter()
        .map(|(k, v)| (k.as_str(), v))
        .collect();
    if entries.is_empty() {
        if let Some(snapshot) = result.rate_limits.as_ref() {
            entries.push((snapshot.limit_id.as_deref().unwrap_or("codex"), snapshot));
        }
    }
    // "codex" first so its plan/credits win when several limits carry them.
    entries.sort_by_key(|(key, _)| (*key != "codex", *key));

    for (key, snapshot) in entries {
        let limit_id = snapshot.limit_id.as_deref().unwrap_or(key);
        windows.extend(map_rate_limit_snapshot(
            limit_id,
            snapshot,
            observed_at,
            "app-server",
        ));
        if plan.is_none() {
            plan = snapshot.plan_type.as_deref().map(plan_label);
        }
        if credits.is_none() {
            if let Some(c) = snapshot.credits.as_ref() {
                credits = Some(map_credits(c, result.rate_limit_reset_credits.as_ref()));
            }
        }
    }
    (windows, credits, plan)
}

/// Apply rollover to a window whose `resets_at` has already passed: zero the
/// usage, clear the reset time and mark the source, so stale rollout-derived
/// data isn't shown as still-elevated usage past its own reset. Same
/// semantics as `ts_claude::state::apply_rollover`.
pub fn apply_rollover(window: UsageWindow, now: i64) -> UsageWindow {
    match window.resets_at {
        Some(r) if r <= now => UsageWindow {
            used_percent: 0.0,
            resets_at: None,
            source: "rollover".to_string(),
            ..window
        },
        _ => window,
    }
}

/// Credits row from a rate-limit snapshot's credits plus the reset-credits summary.
pub fn map_credits(
    credits: &CreditsDto,
    reset_credits: Option<&RateLimitResetCreditsDto>,
) -> Credits {
    let detail = if credits.unlimited {
        Some("Unlimited".to_string())
    } else if let Some(balance) = credits
        .balance
        .as_ref()
        .filter(|b| !b.is_empty() && b.as_str() != "0")
    {
        Some(format!("Balance: {balance}"))
    } else {
        match reset_credits.and_then(|r| r.available_count).unwrap_or(0) {
            0 => None,
            n => Some(format!("{n} reset credit(s) available")),
        }
    };
    Credits {
        label: "Credits".to_string(),
        enabled: credits.has_credits || credits.unlimited,
        used: None,
        limit: None,
        currency: None,
        percent: None,
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One rolled-over `codex:primary` window with the given `used_percent`/`resets_at`.
    fn rolled_primary(used_percent: f64, resets_at: i64, now: i64) -> UsageWindow {
        let snapshot = RateLimitSnapshotDto {
            limit_id: Some("codex".into()),
            primary: Some(crate::dto::RateLimitWindowDto {
                used_percent,
                window_duration_mins: Some(10080),
                resets_at: Some(resets_at),
            }),
            ..Default::default()
        };
        map_rate_limit_snapshot("codex", &snapshot, 100, "rollout")
            .into_iter()
            .map(|w| apply_rollover(w, now))
            .next()
            .unwrap()
    }

    #[test]
    fn rollover_zeroes_usage_for_windows_whose_reset_has_passed() {
        let rolled = rolled_primary(90.0, 400, 500);
        assert_eq!(rolled.used_percent, 0.0);
        assert_eq!(rolled.resets_at, None);
        assert_eq!(rolled.source, "rollover");
    }

    #[test]
    fn rollover_leaves_future_resets_untouched() {
        let rolled = rolled_primary(10.0, 9_999, 500);
        assert_eq!(rolled.used_percent, 10.0);
        assert_eq!(rolled.source, "rollout");
    }

    #[test]
    fn plan_labels_known_and_unknown_types() {
        assert_eq!(plan_label("pro"), "Pro");
        assert_eq!(plan_label("prolite"), "Pro Lite");
        assert_eq!(
            plan_label("self_serve_business_prolite"),
            "Self Serve Business Prolite"
        );
    }

    #[test]
    fn base_labels_by_duration() {
        assert_eq!(base_label(Some(300)), "Session");
        assert_eq!(base_label(Some(10080)), "Weekly");
        assert_eq!(base_label(Some(60)), "1-hour window");
        assert_eq!(base_label(Some(2880)), "2-day window");
        assert_eq!(base_label(None), "Window");
    }

    #[test]
    fn window_label_prefixes_non_codex_limits() {
        assert_eq!(window_label("codex", None, None, Some(10080)), "Weekly");
        assert_eq!(
            window_label("spark", None, Some("gpt-5.3-codex-spark"), Some(10080)),
            "Weekly · gpt-5.3-codex-spark"
        );
        assert_eq!(
            window_label(
                "spark",
                Some("Spark"),
                Some("gpt-5.3-codex-spark"),
                Some(10080)
            ),
            "Weekly · Spark"
        );
    }

    #[test]
    fn window_kind_classification() {
        assert_eq!(window_kind("codex", Some(300)), WindowKind::Session);
        assert_eq!(window_kind("codex", Some(10080)), WindowKind::Weekly);
        assert_eq!(window_kind("spark", Some(10080)), WindowKind::Model);
        assert_eq!(window_kind("codex", None), WindowKind::Other);
    }

    #[test]
    fn maps_primary_only_snapshot_from_fixture() {
        let snapshot = RateLimitSnapshotDto {
            limit_id: Some("codex".into()),
            primary: Some(crate::dto::RateLimitWindowDto {
                used_percent: 12.5,
                window_duration_mins: Some(10080),
                resets_at: Some(1_791_199_753),
            }),
            secondary: None,
            ..Default::default()
        };
        let windows = map_rate_limit_snapshot("codex", &snapshot, 1_790_000_000, "app-server");
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id, "codex:primary");
        assert_eq!(windows[0].label, "Weekly");
        assert_eq!(windows[0].kind, WindowKind::Weekly);
        assert_eq!(windows[0].used_percent, 12.5);
        assert_eq!(windows[0].source, "app-server");
    }

    #[test]
    fn maps_both_windows_when_present() {
        let snapshot = RateLimitSnapshotDto {
            limit_id: Some("codex".into()),
            primary: Some(crate::dto::RateLimitWindowDto {
                used_percent: 1.0,
                window_duration_mins: Some(300),
                resets_at: Some(1),
            }),
            secondary: Some(crate::dto::RateLimitWindowDto {
                used_percent: 2.0,
                window_duration_mins: Some(10080),
                resets_at: Some(2),
            }),
            ..Default::default()
        };
        let windows = map_rate_limit_snapshot("codex", &snapshot, 0, "app-server");
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].id, "codex:primary");
        assert_eq!(windows[1].id, "codex:secondary");
    }

    #[test]
    fn credits_detail_prefers_unlimited_then_balance_then_reset_count() {
        let unlimited = CreditsDto {
            has_credits: true,
            unlimited: true,
            balance: Some("5".into()),
        };
        assert_eq!(
            map_credits(&unlimited, None).detail,
            Some("Unlimited".into())
        );

        let balance = CreditsDto {
            has_credits: true,
            unlimited: false,
            balance: Some("42".into()),
        };
        assert_eq!(
            map_credits(&balance, None).detail,
            Some("Balance: 42".into())
        );

        let reset_only = CreditsDto {
            has_credits: false,
            unlimited: false,
            balance: None,
        };
        let reset = RateLimitResetCreditsDto {
            available_count: Some(3),
        };
        assert_eq!(
            map_credits(&reset_only, Some(&reset)).detail,
            Some("3 reset credit(s) available".into())
        );
        assert!(!map_credits(&reset_only, Some(&reset)).enabled);
    }

    #[test]
    fn credits_from_fixture_has_no_detail() {
        let raw = include_str!("../tests/fixtures/rate_limits_read_response.json");
        let value: serde_json::Value = serde_json::from_str(raw).unwrap();
        let result: crate::dto::RateLimitsReadResult =
            serde_json::from_value(value["result"].clone()).unwrap();
        let codex = result.rate_limits_by_limit_id.get("codex").unwrap();
        let credits = map_credits(
            codex.credits.as_ref().unwrap(),
            result.rate_limit_reset_credits.as_ref(),
        );
        assert!(!credits.enabled);
        assert_eq!(credits.detail, Some("1 reset credit(s) available".into()));
    }

    #[test]
    fn map_rate_limits_uses_map_key_when_limit_id_is_null() {
        let raw = include_str!("../tests/fixtures/rate_limits_read_response.json");
        let value: serde_json::Value = serde_json::from_str(raw).unwrap();
        let result: crate::dto::RateLimitsReadResult =
            serde_json::from_value(value["result"].clone()).unwrap();
        let (windows, credits, plan) = map_rate_limits(&result, 1_790_000_000);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id, "codex:primary");
        assert_eq!(windows[0].kind, WindowKind::Weekly);
        assert_eq!(plan, Some("Pro".to_string()));
        assert_eq!(
            credits.unwrap().detail,
            Some("1 reset credit(s) available".into())
        );
    }

    #[test]
    fn map_rate_limits_falls_back_to_single_snapshot() {
        let result = RateLimitsReadResult {
            rate_limits: Some(RateLimitSnapshotDto {
                limit_id: None,
                plan_type: Some("plus".into()),
                primary: Some(crate::dto::RateLimitWindowDto {
                    used_percent: 5.0,
                    window_duration_mins: Some(300),
                    resets_at: Some(1),
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let (windows, _, plan) = map_rate_limits(&result, 0);
        assert_eq!(windows[0].id, "codex:primary");
        assert_eq!(plan, Some("Plus".to_string()));
    }
}
