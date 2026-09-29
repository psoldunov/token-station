//! Timestamp helpers for the loosely typed JSON the CLIs emit.

use chrono::DateTime;
use serde_json::Value;

/// Epoch values above this are treated as milliseconds.
const MILLIS_THRESHOLD: i64 = 100_000_000_000;

/// Parse an RFC 3339 / ISO 8601 string, epoch seconds or epoch milliseconds into
/// Unix seconds.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "a fractional epoch is truncated to whole seconds on purpose; `as` saturates out-of-range values"
)]
pub fn parse_timestamp(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => {
            let raw = n.as_i64().or_else(|| n.as_f64().map(|f| f as i64))?;
            Some(normalize_epoch(raw))
        }
        Value::String(s) => parse_timestamp_str(s),
        _ => None,
    }
}

/// Parse an RFC 3339 string or a numeric string.
pub fn parse_timestamp_str(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp());
    }
    s.parse::<i64>().ok().map(normalize_epoch)
}

fn normalize_epoch(raw: i64) -> i64 {
    if raw > MILLIS_THRESHOLD {
        raw / 1000
    } else {
        raw
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_all_supported_forms() {
        assert_eq!(
            parse_timestamp(&json!("2026-09-28T23:29:59.688671+00:00")),
            Some(1_790_638_199)
        );
        assert_eq!(
            parse_timestamp(&json!("2026-09-28T12:00:00Z")),
            Some(1_790_596_800)
        );
        assert_eq!(parse_timestamp(&json!(1_790_596_800)), Some(1_790_596_800));
        assert_eq!(
            parse_timestamp(&json!(1_790_596_800_123_i64)),
            Some(1_790_596_800)
        );
        assert_eq!(
            parse_timestamp(&json!(1_790_596_800.9)),
            Some(1_790_596_800)
        );
        assert_eq!(parse_timestamp(&json!("1790596800")), Some(1_790_596_800));
    }

    #[test]
    fn rejects_other_values() {
        assert_eq!(parse_timestamp(&json!(null)), None);
        assert_eq!(parse_timestamp(&json!("soon")), None);
        assert_eq!(parse_timestamp(&json!({"a": 1})), None);
    }
}
