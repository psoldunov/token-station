//! Maps `account/usage/read` into the backend-wide [`AccountTokens`] shape.

use std::collections::HashMap;

use chrono::{DateTime, Days, TimeZone, Utc};
use ts_core::snapshot::{AccountTokens, DailyTokens};

use crate::dto::UsageReadResult;

/// `today`/`last7Days` use the local calendar date of `now` in `tz`; `daily`
/// is the last 14 local calendar days, oldest first, zero-filled.
pub fn map_account_tokens<Tz: TimeZone>(
    result: &UsageReadResult,
    now: DateTime<Utc>,
    tz: &Tz,
) -> AccountTokens {
    let today = now.with_timezone(tz).date_naive();
    let by_date: HashMap<&str, u64> = result
        .daily_usage_buckets
        .iter()
        .map(|b| (b.start_date.as_str(), b.tokens))
        .collect();

    let tokens_on = |offset: u64| -> u64 {
        today
            .checked_sub_days(Days::new(offset))
            .and_then(|d| {
                by_date
                    .get(d.format("%Y-%m-%d").to_string().as_str())
                    .copied()
            })
            .unwrap_or(0)
    };

    let today_tokens = tokens_on(0);
    let last7_days = (0..7).map(tokens_on).sum();
    let daily = (0..14)
        .rev()
        .filter_map(|offset| today.checked_sub_days(Days::new(offset)))
        .map(|date| {
            let key = date.format("%Y-%m-%d").to_string();
            let tokens = by_date.get(key.as_str()).copied().unwrap_or(0);
            DailyTokens { date: key, tokens }
        })
        .collect();

    AccountTokens {
        today: today_tokens,
        last7_days,
        lifetime: result.summary.as_ref().and_then(|s| s.lifetime_tokens),
        daily,
        updated_at: now.timestamp(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::{DailyBucketDto, UsageSummaryDto};

    fn result() -> UsageReadResult {
        UsageReadResult {
            summary: Some(UsageSummaryDto {
                lifetime_tokens: Some(4_743_603_614),
            }),
            daily_usage_buckets: vec![
                DailyBucketDto {
                    start_date: "2026-09-22".into(),
                    tokens: 1_020_000,
                },
                DailyBucketDto {
                    start_date: "2026-09-25".into(),
                    tokens: 980_223,
                },
                DailyBucketDto {
                    start_date: "2026-09-27".into(),
                    tokens: 2_600_466,
                },
                DailyBucketDto {
                    start_date: "2026-09-28".into(),
                    tokens: 1_204_331,
                },
            ],
        }
    }

    #[test]
    fn maps_fixture_buckets_against_utc_today() {
        let now = DateTime::from_timestamp(1_790_596_800, 0).unwrap(); // 2026-09-28T12:00:00Z
        let mapped = map_account_tokens(&result(), now, &Utc);
        assert_eq!(mapped.today, 1_204_331);
        assert_eq!(
            mapped.last7_days,
            1_020_000 + 980_223 + 2_600_466 + 1_204_331
        );
        assert_eq!(mapped.lifetime, Some(4_743_603_614));
        assert_eq!(mapped.daily.len(), 14);
        assert_eq!(mapped.daily[0].date, "2026-09-15");
        assert_eq!(mapped.daily.last().unwrap().date, "2026-09-28");
        assert_eq!(mapped.daily.last().unwrap().tokens, 1_204_331);
        assert_eq!(mapped.daily[0].tokens, 0);
    }

    #[test]
    fn empty_result_is_all_zero() {
        let now = DateTime::from_timestamp(0, 0).unwrap();
        let mapped = map_account_tokens(&UsageReadResult::default(), now, &Utc);
        assert_eq!(mapped.today, 0);
        assert_eq!(mapped.last7_days, 0);
        assert_eq!(mapped.lifetime, None);
        assert_eq!(mapped.daily.len(), 14);
    }
}
