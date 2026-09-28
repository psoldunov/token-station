//! Token accounting from local CLI logs.
//!
//! Providers scan their logs into [`TokenEvent`]s; a [`TokenLedger`] de-duplicates
//! them, keeps a bounded time window and aggregates reports.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Days, NaiveDate, TimeZone, Utc};

use crate::pricing::Pricing;
use crate::snapshot::{ModelTotals, TokenReport, TokenTotals};

/// Token counts for one request, in disjoint buckets.
///
/// Providers must normalize to these semantics: `input` excludes cached input,
/// `cache_read`/`cache_write` are cache hits/creations, `output` includes
/// reasoning, and `reasoning` is the informational subset of `output`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenCounts {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub reasoning: u64,
}

impl TokenCounts {
    pub fn total(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }

    /// Prompt-side tokens, used to pick the long-context price tier.
    pub fn prompt_tokens(&self) -> u64 {
        self.input
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }

    pub fn plus(&self, other: &TokenCounts) -> TokenCounts {
        TokenCounts {
            input: self.input.saturating_add(other.input),
            output: self.output.saturating_add(other.output),
            cache_read: self.cache_read.saturating_add(other.cache_read),
            cache_write: self.cache_write.saturating_add(other.cache_write),
            reasoning: self.reasoning.saturating_add(other.reasoning),
        }
    }
}

/// One billed request found in a local log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenEvent {
    /// Unix seconds.
    pub timestamp: i64,
    pub model: String,
    pub counts: TokenCounts,
    /// Unique per request; duplicates (retries, re-logged messages) are dropped.
    pub dedup_key: String,
}

/// De-duplicated events within a retention window.
#[derive(Debug, Clone, Default)]
pub struct TokenLedger {
    events: HashMap<String, TokenEvent>,
}

impl TokenLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Ledger with `events` added. When an event's key is already present, the
    /// event with the larger total token count wins: streamed content-block
    /// lines for the same request can carry partial usage before the final
    /// line reports the full count, and the smaller one must not shadow it.
    pub fn with_events(self, events: impl IntoIterator<Item = TokenEvent>) -> TokenLedger {
        let mut map = self.events;
        for event in events {
            map.entry(event.dedup_key.clone())
                .and_modify(|existing| {
                    if event.counts.total() > existing.counts.total() {
                        *existing = event.clone();
                    }
                })
                .or_insert(event);
        }
        TokenLedger { events: map }
    }

    /// Ledger without events older than `cutoff` (Unix seconds).
    pub fn pruned(self, cutoff: i64) -> TokenLedger {
        TokenLedger {
            events: self
                .events
                .into_iter()
                .filter(|(_, e)| e.timestamp >= cutoff)
                .collect(),
        }
    }

    /// Aggregate today (local calendar day of `now` in `tz`) and the last 7 local days.
    pub fn report<Tz: TimeZone>(
        &self,
        now: DateTime<Utc>,
        tz: &Tz,
        pricing: &Pricing,
    ) -> TokenReport {
        let today = now.with_timezone(tz).date_naive();
        let today_start = local_day_start(tz, today);
        let week_start = today
            .checked_sub_days(Days::new(6))
            .map_or(today_start, |d| local_day_start(tz, d));

        let today_events = self.events.values().filter(|e| e.timestamp >= today_start);
        let week_events: Vec<&TokenEvent> = self
            .events
            .values()
            .filter(|e| e.timestamp >= week_start)
            .collect();

        let mut per_model: BTreeMap<&str, Vec<&TokenEvent>> = BTreeMap::new();
        for e in &week_events {
            per_model.entry(e.model.as_str()).or_default().push(e);
        }
        let mut by_model: Vec<ModelTotals> = per_model
            .into_iter()
            .map(|(model, events)| ModelTotals {
                model: model.to_string(),
                totals: totals(events.into_iter(), pricing),
            })
            .collect();
        by_model.sort_by(|a, b| {
            let cost = |m: &ModelTotals| m.totals.cost_usd.unwrap_or(0.0);
            cost(b)
                .total_cmp(&cost(a))
                .then(b.totals.total.cmp(&a.totals.total))
        });

        TokenReport {
            today: totals(today_events, pricing),
            last7_days: totals(week_events.into_iter(), pricing),
            by_model,
            updated_at: now.timestamp(),
        }
    }
}

fn totals<'a>(events: impl Iterator<Item = &'a TokenEvent>, pricing: &Pricing) -> TokenTotals {
    events.fold(TokenTotals::default(), |acc, e| {
        let c = &e.counts;
        let cost = pricing.cost(&e.model, c);
        TokenTotals {
            input: acc.input.saturating_add(c.input),
            output: acc.output.saturating_add(c.output),
            cache_read: acc.cache_read.saturating_add(c.cache_read),
            cache_write: acc.cache_write.saturating_add(c.cache_write),
            reasoning: acc.reasoning.saturating_add(c.reasoning),
            total: acc.total.saturating_add(c.total()),
            cost_usd: match (acc.cost_usd, cost) {
                (Some(a), Some(b)) => Some(a + b),
                (a, b) => a.or(b),
            },
            unpriced_tokens: acc.unpriced_tokens.saturating_add(if cost.is_none() {
                c.total()
            } else {
                0
            }),
            requests: acc.requests.saturating_add(1),
        }
    })
}

/// Unix seconds of local midnight starting `date` (earliest instant for DST gaps).
pub fn local_day_start<Tz: TimeZone>(tz: &Tz, date: NaiveDate) -> i64 {
    let midnight = date.and_hms_opt(0, 0, 0).unwrap_or_default();
    tz.from_local_datetime(&midnight)
        .earliest()
        .map_or_else(|| midnight.and_utc().timestamp(), |dt| dt.timestamp())
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::*;

    fn ev(key: &str, ts: i64, model: &str, input: u64, output: u64) -> TokenEvent {
        TokenEvent {
            timestamp: ts,
            model: model.into(),
            counts: TokenCounts {
                input,
                output,
                ..TokenCounts::default()
            },
            dedup_key: key.into(),
        }
    }

    fn pricing() -> Pricing {
        Pricing::from_litellm_json(
            r#"{"m-a": {"input_cost_per_token": 1.0, "output_cost_per_token": 2.0}}"#,
        )
        .unwrap()
    }

    // 2026-09-28 12:00:00 UTC
    const NOON: i64 = 1_790_596_800;

    #[test]
    fn dedups_by_key_keeping_larger_total_regardless_of_arrival_order() {
        // Smaller event arrives first, larger second (across two `with_events` calls).
        let smaller_first = TokenLedger::new()
            .with_events([ev("k1", NOON, "m-a", 1, 1), ev("k1", NOON, "m-a", 99, 99)])
            .with_events([ev("k2", NOON, "m-a", 1, 0)]);
        assert_eq!(smaller_first.len(), 2);
        let r = smaller_first.report(DateTime::from_timestamp(NOON, 0).unwrap(), &Utc, &pricing());
        assert_eq!(r.today.input, 100); // 99 (larger total) + 1, not the smaller first event
        assert_eq!(r.today.requests, 2);

        // Larger event arrives first, smaller second (single `with_events` call):
        // the smaller must not overwrite it either.
        let larger_first = TokenLedger::new()
            .with_events([ev("k1", NOON, "m-a", 99, 99), ev("k1", NOON, "m-a", 1, 1)]);
        assert_eq!(larger_first.len(), 1);
        let r2 = larger_first.report(DateTime::from_timestamp(NOON, 0).unwrap(), &Utc, &pricing());
        assert_eq!(r2.today.input, 99);
    }

    #[test]
    fn splits_today_and_week_in_local_time() {
        let tz = FixedOffset::east_opt(3 * 3600).unwrap(); // UTC+3
        let now = DateTime::from_timestamp(NOON, 0).unwrap(); // 15:00 local
        let l = TokenLedger::new().with_events([
            ev("today", NOON - 3600, "m-a", 10, 0),
            // 20:30 UTC previous day = 23:30 local previous day -> not today
            ev("yesterday", NOON - 15 * 3600 - 1800, "m-a", 20, 0),
            ev("six-days", NOON - 6 * 86_400, "m-a", 40, 0),
            ev("eight-days", NOON - 8 * 86_400, "m-a", 80, 0),
        ]);
        let r = l.report(now, &tz, &pricing());
        assert_eq!(r.today.input, 10);
        assert_eq!(r.last7_days.input, 70);
        assert_eq!(r.updated_at, NOON);
    }

    #[test]
    fn prices_and_tracks_unpriced_tokens() {
        let l = TokenLedger::new().with_events([
            ev("a", NOON, "m-a", 3, 1),
            ev("b", NOON, "unknown-model", 5, 5),
        ]);
        let r = l.report(DateTime::from_timestamp(NOON, 0).unwrap(), &Utc, &pricing());
        assert_eq!(r.today.cost_usd, Some(3.0 * 1.0 + 1.0 * 2.0));
        assert_eq!(r.today.unpriced_tokens, 10);
        assert_eq!(r.today.total, 14);
        assert_eq!(r.by_model[0].model, "m-a");
        assert_eq!(r.by_model[1].totals.cost_usd, None);
    }

    #[test]
    fn empty_ledger_reports_zero_without_cost() {
        let r =
            TokenLedger::new().report(DateTime::from_timestamp(NOON, 0).unwrap(), &Utc, &pricing());
        assert_eq!(r.today, TokenTotals::default());
        assert!(r.by_model.is_empty());
    }

    #[test]
    fn pruned_drops_old_events() {
        let l = TokenLedger::new()
            .with_events([ev("old", 10, "m-a", 1, 1), ev("new", 100, "m-a", 1, 1)])
            .pruned(50);
        assert_eq!(l.len(), 1);
    }

    #[test]
    fn counts_total_excludes_reasoning() {
        let c = TokenCounts {
            input: 1,
            output: 10,
            cache_read: 100,
            cache_write: 1000,
            reasoning: 7,
        };
        assert_eq!(c.total(), 1111);
        assert_eq!(c.prompt_tokens(), 1101);
        assert_eq!(c.plus(&c).reasoning, 14);
    }
}
