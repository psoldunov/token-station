//! Parses the statusline JSON Claude Code pipes to `token-station statusline`.

use serde_json::Value;
use ts_core::snapshot::{Level, UsageWindow, WindowKind};

/// One rate-limit observation extracted from `rate_limits.{five_hour,seven_day}`.
#[derive(Debug, Clone, PartialEq)]
pub struct StatuslineObservation {
    pub session: Option<UsageWindow>,
    pub weekly_all: Option<UsageWindow>,
}

/// Parse the statusline payload. `Ok(None)` means valid JSON with no `rate_limits`
/// (not signed into a plan with limits, or first response of the session).
pub fn parse(text: &str, now: i64) -> Result<Option<StatuslineObservation>, String> {
    let root: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let Some(rate_limits) = root.get("rate_limits") else {
        return Ok(None);
    };
    let session = window(
        rate_limits.get("five_hour"),
        "session",
        "Session",
        WindowKind::Session,
        Some(300),
        now,
    );
    let weekly_all = window(
        rate_limits.get("seven_day"),
        "weekly_all",
        "Weekly",
        WindowKind::Weekly,
        Some(10080),
        now,
    );
    Ok(Some(StatuslineObservation {
        session,
        weekly_all,
    }))
}

fn window(
    value: Option<&Value>,
    id: &str,
    label: &str,
    kind: WindowKind,
    minutes: Option<u32>,
    now: i64,
) -> Option<UsageWindow> {
    let value = value?;
    let used_percent = value.get("used_percentage").and_then(Value::as_f64)?;
    let resets_at = value
        .get("resets_at")
        .and_then(ts_core::time::parse_timestamp);
    Some(UsageWindow {
        id: id.to_string(),
        label: label.to_string(),
        kind,
        used_percent,
        resets_at,
        window_minutes: minutes,
        level: Level::default(),
        source: "statusline".to_string(),
        observed_at: now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/statusline_input.json");
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn parses_real_fixture() {
        let obs = parse(&fixture(), 1000).unwrap().unwrap();
        let session = obs.session.unwrap();
        assert_eq!(session.used_percent, 34.0);
        assert_eq!(session.resets_at, Some(1_790_638_199));
        assert_eq!(session.source, "statusline");
        let weekly = obs.weekly_all.unwrap();
        assert_eq!(weekly.used_percent, 15.0);
        assert_eq!(weekly.resets_at, Some(1_791_003_599));
    }

    #[test]
    fn missing_rate_limits_is_ok_none() {
        assert_eq!(parse(r#"{"hook_event_name":"Status"}"#, 0).unwrap(), None);
    }

    #[test]
    fn invalid_json_is_an_error() {
        assert!(parse("not json", 0).is_err());
    }
}
