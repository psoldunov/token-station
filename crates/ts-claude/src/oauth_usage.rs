//! `GET /api/oauth/usage`: HTTP client and lenient response parser.

use std::time::Duration;

use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;
use ts_core::snapshot::{BreakdownRow, Credits, UsageWindow, WindowKind};

use crate::labels::humanize;

/// Parsed plan-limit data from one successful `oauth/usage` call.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedUsage {
    pub windows: Vec<UsageWindow>,
    pub credits: Option<Credits>,
    pub breakdown: Vec<BreakdownRow>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("response is not valid JSON: {0}")]
    Json(String),
}

/// Parse the endpoint body. Never panics: any field that is missing or the
/// wrong shape is simply omitted rather than treated as an error.
pub fn parse_usage(text: &str, now: i64) -> Result<ParsedUsage, ParseError> {
    let root: Value = serde_json::from_str(text).map_err(|e| ParseError::Json(e.to_string()))?;

    let windows = match root.get("limits").and_then(Value::as_array) {
        Some(arr) if !arr.is_empty() => parse_limits(arr, now),
        _ => parse_fallback(&root, now),
    };
    let credits = credits_from_spend(root.get("spend"))
        .or_else(|| credits_from_extra_usage(root.get("extra_usage")));
    let breakdown = parse_breakdown(root.get("seven_day_breakdown"));

    Ok(ParsedUsage {
        windows,
        credits,
        breakdown,
    })
}

fn parse_limits(items: &[Value], now: i64) -> Vec<UsageWindow> {
    items
        .iter()
        .filter_map(|item| {
            let kind = item.get("kind").and_then(Value::as_str)?;
            if kind.trim().is_empty() {
                return None;
            }
            let percent = item.get("percent").and_then(Value::as_f64).unwrap_or(0.0);
            let resets_at = item
                .get("resets_at")
                .and_then(ts_core::time::parse_timestamp);
            let (id, label, window_kind, minutes) = match kind {
                "session" => (
                    "session".to_string(),
                    "Session".to_string(),
                    WindowKind::Session,
                    Some(300),
                ),
                "weekly_all" => (
                    "weekly_all".to_string(),
                    "Weekly".to_string(),
                    WindowKind::Weekly,
                    Some(10080),
                ),
                "weekly_scoped" => weekly_scoped_fields(item),
                other => (other.to_string(), humanize(other), WindowKind::Other, None),
            };
            Some(UsageWindow {
                id,
                label,
                kind: window_kind,
                used_percent: percent,
                resets_at,
                window_minutes: minutes,
                level: ts_core::snapshot::Level::default(),
                source: "oauth".to_string(),
                observed_at: now,
            })
        })
        .collect()
}

fn weekly_scoped_fields(item: &Value) -> (String, String, WindowKind, Option<u32>) {
    let display_name = item
        .get("scope")
        .and_then(|s| s.get("model"))
        .and_then(|m| m.get("display_name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match display_name {
        Some(name) => (
            format!("weekly_scoped:{}", name.to_ascii_lowercase()),
            format!("Weekly · {name}"),
            WindowKind::Model,
            Some(10080),
        ),
        None => (
            "weekly_scoped".to_string(),
            "Weekly · Unknown".to_string(),
            WindowKind::Model,
            Some(10080),
        ),
    }
}

/// Known fallback keys, in the order the spec lists them.
const FALLBACK_KNOWN: &[(&str, &str, &str, WindowKind, Option<u32>)] = &[
    (
        "five_hour",
        "session",
        "Session",
        WindowKind::Session,
        Some(300),
    ),
    (
        "seven_day",
        "weekly_all",
        "Weekly",
        WindowKind::Weekly,
        Some(10080),
    ),
    (
        "seven_day_opus",
        "weekly_scoped:opus",
        "Weekly · Opus",
        WindowKind::Model,
        Some(10080),
    ),
    (
        "seven_day_sonnet",
        "weekly_scoped:sonnet",
        "Weekly · Sonnet",
        WindowKind::Model,
        Some(10080),
    ),
];

const FALLBACK_EXCLUDED: &[&str] = &[
    "five_hour",
    "seven_day",
    "seven_day_opus",
    "seven_day_sonnet",
    "limits",
    "spend",
    "extra_usage",
    "seven_day_breakdown",
    "member_dashboard_available",
];

fn parse_fallback(root: &Value, now: i64) -> Vec<UsageWindow> {
    let mut out = Vec::new();
    for (key, id, label, kind, minutes) in FALLBACK_KNOWN {
        if let Some(w) = keyed_window(root.get(key), id, label, *kind, *minutes, now) {
            out.push(w);
        }
    }

    let Some(obj) = root.as_object() else {
        return out;
    };
    let mut other_keys: Vec<&String> = obj
        .keys()
        .filter(|k| !FALLBACK_EXCLUDED.contains(&k.as_str()))
        .collect();
    other_keys.sort();
    for key in other_keys {
        if let Some(w) = keyed_window(
            obj.get(key),
            key,
            &humanize(key),
            WindowKind::Other,
            None,
            now,
        ) {
            out.push(w);
        }
    }
    out
}

fn keyed_window(
    value: Option<&Value>,
    id: &str,
    label: &str,
    kind: WindowKind,
    minutes: Option<u32>,
    now: i64,
) -> Option<UsageWindow> {
    let value = value?;
    let utilization = value.get("utilization").and_then(Value::as_f64)?;
    let resets_at = value
        .get("resets_at")
        .and_then(ts_core::time::parse_timestamp)?;
    Some(UsageWindow {
        id: id.to_string(),
        label: label.to_string(),
        kind,
        used_percent: utilization,
        resets_at: Some(resets_at),
        window_minutes: minutes,
        level: ts_core::snapshot::Level::default(),
        source: "oauth".to_string(),
        observed_at: now,
    })
}

fn as_flexible_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// Shared tail of `credits_from_spend`/`credits_from_extra_usage`: both
/// sources map to the same [`Credits`] shape once their differently-named
/// fields are extracted.
fn credits_from_fields(
    enabled: bool,
    used: Option<f64>,
    limit: Option<f64>,
    percent: Option<f64>,
    currency: Option<String>,
) -> Credits {
    Credits {
        label: "Extra usage".to_string(),
        enabled,
        used,
        limit,
        currency,
        percent,
        detail: (!enabled).then(|| "Turned off".to_string()),
    }
}

fn credits_from_spend(spend: Option<&Value>) -> Option<Credits> {
    let spend = spend.filter(|v| !v.is_null())?;
    let enabled = spend
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let used_obj = spend.get("used").filter(|v| !v.is_null());
    let used = used_obj.and_then(|u| {
        let amount = u.get("amount_minor").and_then(as_flexible_f64)?;
        let exponent = u.get("exponent").and_then(as_flexible_f64).unwrap_or(0.0);
        Some(amount / 10f64.powf(exponent))
    });
    let currency = used_obj
        .and_then(|u| u.get("currency"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let limit = spend.get("limit").and_then(as_flexible_f64);
    let percent = spend.get("percent").and_then(as_flexible_f64);
    Some(credits_from_fields(enabled, used, limit, percent, currency))
}

fn credits_from_extra_usage(extra: Option<&Value>) -> Option<Credits> {
    let extra = extra.filter(|v| !v.is_null())?;
    let enabled = extra
        .get("is_enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let used = extra.get("used_credits").and_then(as_flexible_f64);
    let limit = extra.get("monthly_limit").and_then(as_flexible_f64);
    let percent = extra.get("utilization").and_then(as_flexible_f64);
    let currency = extra
        .get("currency")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(credits_from_fields(enabled, used, limit, percent, currency))
}

fn parse_breakdown(breakdown: Option<&Value>) -> Vec<BreakdownRow> {
    let Some(rows) = breakdown
        .and_then(|b| b.get("rows"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let key = row.get("key").and_then(Value::as_str)?.to_string();
            let label = row.get("display_name").and_then(Value::as_str)?.to_string();
            let percent = row.get("percent").and_then(Value::as_f64)?;
            Some(BreakdownRow {
                key,
                label,
                percent,
            })
        })
        .collect()
}

/// Outcome of one HTTP attempt against the usage endpoint.
#[derive(Debug)]
pub enum FetchOutcome {
    Ok(String),
    Unauthorized,
    Forbidden,
    RateLimited { retry_after_secs: Option<u64> },
    ServerError(u16),
    Network(String),
}

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Call `GET {base_url}/api/oauth/usage` with the OAuth bearer token.
pub async fn fetch(
    client: &reqwest::Client,
    base_url: &str,
    token: &SecretString,
    user_agent: &str,
    now: i64,
) -> FetchOutcome {
    let url = format!("{}/api/oauth/usage", base_url.trim_end_matches('/'));
    let request = client
        .get(url)
        .bearer_auth(token.expose_secret())
        .header("anthropic-beta", "oauth-2025-04-20")
        .header(reqwest::header::USER_AGENT, user_agent)
        .header(reqwest::header::ACCEPT, "application/json")
        .timeout(REQUEST_TIMEOUT);

    let response = match request.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "claude oauth usage request failed");
            return FetchOutcome::Network(e.to_string());
        }
    };
    let status = response.status().as_u16();
    match status {
        200 => match response.text().await {
            Ok(body) => FetchOutcome::Ok(body),
            Err(e) => {
                tracing::warn!(error = %e, "claude oauth usage response body read failed");
                FetchOutcome::Network(e.to_string())
            }
        },
        401 => FetchOutcome::Unauthorized,
        403 => FetchOutcome::Forbidden,
        429 => {
            let retry_after_secs = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| parse_retry_after_secs(v, now));
            FetchOutcome::RateLimited { retry_after_secs }
        }
        other => FetchOutcome::ServerError(other),
    }
}

fn parse_retry_after_secs(value: &str, now: i64) -> Option<u64> {
    let trimmed = value.trim();
    if let Ok(secs) = trimmed.parse::<u64>() {
        return Some(secs);
    }
    let fmt = "%a, %d %b %Y %H:%M:%S GMT";
    chrono::NaiveDateTime::parse_from_str(trimmed, fmt)
        .ok()
        .map(|naive| u64::try_from(naive.and_utc().timestamp() - now).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "compares exact literals that never went through arithmetic"
    )]
    fn parses_real_fixture_via_limits() {
        let text = fixture("oauth_usage_max_2026-09.json");
        let parsed = parse_usage(&text, 1_790_596_800).unwrap();
        assert_eq!(parsed.windows.len(), 3);
        assert_eq!(parsed.windows[0].id, "session");
        assert_eq!(parsed.windows[0].used_percent, 12.0);
        assert_eq!(parsed.windows[1].id, "weekly_all");
        assert_eq!(parsed.windows[1].used_percent, 15.0);
        assert_eq!(parsed.windows[2].id, "weekly_scoped:fable");
        assert_eq!(parsed.windows[2].label, "Weekly · Fable");
        assert_eq!(parsed.windows[2].used_percent, 0.0);

        let credits = parsed.credits.unwrap();
        assert!(!credits.enabled);
        assert_eq!(credits.detail.as_deref(), Some("Turned off"));

        assert_eq!(parsed.breakdown.len(), 4);
        assert_eq!(parsed.breakdown[0].key, "claude_code");
        assert_eq!(parsed.breakdown[0].percent, 97.0);
    }

    #[test]
    fn falls_back_to_keyed_windows_when_limits_absent() {
        let text = serde_json::json!({
            "five_hour": {"utilization": 34.0, "resets_at": "2026-09-28T23:29:59Z"},
            "seven_day": {"utilization": 15.0, "resets_at": "2026-10-03T00:59:59Z"},
            "seven_day_opus": {"utilization": 5.0, "resets_at": "2026-10-03T00:59:59Z"},
            "seven_day_sonnet": null,
            "mystery_window": {"utilization": 2.0, "resets_at": "2026-10-03T00:59:59Z"},
            "no_reset": {"utilization": 1.0, "resets_at": null},
            "extra_usage": {"is_enabled": false, "used_credits": null, "monthly_limit": null, "utilization": null, "currency": null}
        })
        .to_string();
        let parsed = parse_usage(&text, 0).unwrap();
        let ids: Vec<&str> = parsed.windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "session",
                "weekly_all",
                "weekly_scoped:opus",
                "mystery_window"
            ]
        );
        assert_eq!(parsed.windows[2].label, "Weekly · Opus");
        assert_eq!(parsed.windows[3].label, "Mystery Window");

        let credits = parsed.credits.unwrap();
        assert!(!credits.enabled);
    }

    /// Parse a `limits` array with exactly one entry and return its window.
    fn single_limit_window(limit: &serde_json::Value) -> UsageWindow {
        let text = serde_json::json!({ "limits": [limit] }).to_string();
        let parsed = parse_usage(&text, 0).unwrap();
        parsed.windows.into_iter().next().unwrap()
    }

    #[test]
    fn weekly_scoped_without_display_name_is_graceful() {
        let window = single_limit_window(&serde_json::json!(
            {"kind": "weekly_scoped", "percent": 1.0, "resets_at": null, "scope": null}
        ));
        assert_eq!(window.id, "weekly_scoped");
        assert_eq!(window.label, "Weekly · Unknown");
    }

    #[test]
    fn unknown_limit_kind_is_humanized_other() {
        let window = single_limit_window(
            &serde_json::json!({"kind": "cowork_extra", "percent": 3.0, "resets_at": null}),
        );
        assert_eq!(window.id, "cowork_extra");
        assert_eq!(window.label, "Cowork Extra");
        assert_eq!(window.kind, WindowKind::Other);
    }

    #[test]
    fn invalid_json_is_an_error() {
        assert!(parse_usage("not json", 0).is_err());
    }

    #[test]
    fn empty_object_parses_to_nothing() {
        let parsed = parse_usage("{}", 0).unwrap();
        assert!(parsed.windows.is_empty());
        assert!(parsed.credits.is_none());
        assert!(parsed.breakdown.is_empty());
    }

    #[test]
    fn credits_fall_back_to_extra_usage_when_spend_absent() {
        let text = serde_json::json!({
            "extra_usage": {"is_enabled": true, "used_credits": 5.0, "monthly_limit": 20.0, "utilization": 25.0, "currency": "USD"}
        })
        .to_string();
        let parsed = parse_usage(&text, 0).unwrap();
        let c = parsed.credits.unwrap();
        assert!(c.enabled);
        assert_eq!(c.used, Some(5.0));
        assert_eq!(c.limit, Some(20.0));
        assert_eq!(c.percent, Some(25.0));
        assert_eq!(c.currency.as_deref(), Some("USD"));
        assert!(c.detail.is_none());
    }

    #[test]
    fn retry_after_parses_seconds_and_http_date() {
        assert_eq!(parse_retry_after_secs("120", 0), Some(120));
        assert_eq!(
            parse_retry_after_secs("Thu, 01 Jan 1970 00:02:00 GMT", 0),
            Some(120)
        );
        assert_eq!(parse_retry_after_secs("garbage", 0), None);
    }
}
