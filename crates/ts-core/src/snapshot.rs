//! The snapshot published over D-Bus as JSON (`Snapshot` property).
//!
//! Field names serialize as camelCase. Timestamps are Unix epoch seconds. Bump
//! [`SCHEMA_VERSION`] on any breaking change and regenerate
//! `data/snapshot.schema.json` (see `tests/schema.rs`).

use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Version of the snapshot JSON contract.
pub const SCHEMA_VERSION: u32 = 1;

/// A usage provider (one CLI).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Claude,
    Codex,
}

impl ProviderId {
    /// All providers, in display order (also the order of the tray meter bars).
    pub const ALL: [ProviderId; 2] = [ProviderId::Claude, ProviderId::Codex];

    pub fn as_str(self) -> &'static str {
        match self {
            ProviderId::Claude => "claude",
            ProviderId::Codex => "codex",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            ProviderId::Claude => "Claude Code",
            ProviderId::Codex => "Codex",
        }
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ProviderId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "claude" => Ok(ProviderId::Claude),
            "codex" => Ok(ProviderId::Codex),
            other => Err(format!("unknown provider `{other}`")),
        }
    }
}

/// Severity derived from the configured warning/critical thresholds.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    #[default]
    Normal,
    Warning,
    Critical,
}

impl Level {
    /// Classify a 0–100 percentage against the warning and critical thresholds.
    pub fn from_percent(percent: f64, warning: f64, critical: f64) -> Level {
        if percent >= critical {
            Level::Critical
        } else if percent >= warning {
            Level::Warning
        } else {
            Level::Normal
        }
    }
}

/// Health of one provider's data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderState {
    /// No data yet (first refresh in flight).
    Loading,
    /// Fresh data.
    Ok,
    /// Showing the last good data; `message` says why it could not refresh.
    Stale,
    /// The CLI is installed but not signed in (or the sign-in expired).
    Unauthenticated,
    /// The CLI and its data directory were not found.
    NotInstalled,
    /// Turned off in settings.
    Disabled,
    /// Refresh failed and there is no previous data to show.
    Error,
}

/// What kind of limit a window represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WindowKind {
    /// Short rolling window (e.g. 5 hours).
    Session,
    /// Weekly window across all models.
    Weekly,
    /// Model- or feature-scoped window.
    Model,
    Other,
}

/// One rate-limit window, e.g. "Session · 34 % · resets 23:30".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    /// Stable id, e.g. `session`, `weekly_all`, `weekly_scoped:fable`, `codex:primary`.
    pub id: String,
    /// Human label, e.g. "Session", "Weekly", "Weekly · Fable".
    pub label: String,
    pub kind: WindowKind,
    /// 0–100.
    pub used_percent: f64,
    pub resets_at: Option<i64>,
    pub window_minutes: Option<u32>,
    /// Filled in by [`crate::assemble::assemble`] from the configured thresholds.
    #[serde(default)]
    pub level: Level,
    /// Where the value came from: `oauth`, `statusline`, `app-server`, `http`, `rollout`.
    pub source: String,
    /// When the value was observed.
    pub observed_at: i64,
}

/// Paid overage / credit balance (Claude "extra usage", Codex credits).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Credits {
    pub label: String,
    pub enabled: bool,
    pub used: Option<f64>,
    pub limit: Option<f64>,
    pub currency: Option<String>,
    pub percent: Option<f64>,
    /// Free-form extra line, e.g. "1 reset credit available".
    pub detail: Option<String>,
}

/// Share of the weekly window per surface (Claude only: "Claude Code 97 %").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BreakdownRow {
    pub key: String,
    pub label: String,
    pub percent: f64,
}

/// Token totals in disjoint buckets.
///
/// `total = input + cacheRead + cacheWrite + output`. `reasoning` is a subset of
/// `output`, shown for information only.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenTotals {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub reasoning: u64,
    pub total: u64,
    /// API-equivalent cost of the priced part, if any model was priced.
    pub cost_usd: Option<f64>,
    /// Tokens whose model had no known price.
    pub unpriced_tokens: u64,
    pub requests: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelTotals {
    pub model: String,
    #[serde(flatten)]
    pub totals: TokenTotals,
}

/// Token usage recorded in this machine's local CLI logs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenReport {
    pub today: TokenTotals,
    pub last7_days: TokenTotals,
    /// Last 7 days, most expensive first.
    pub by_model: Vec<ModelTotals>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DailyTokens {
    /// Local calendar date, `YYYY-MM-DD`.
    pub date: String,
    pub tokens: u64,
}

/// Account-wide token usage reported by the provider backend (all devices).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountTokens {
    pub today: u64,
    pub last7_days: u64,
    pub lifetime: Option<u64>,
    /// Oldest first, at most 30 entries.
    pub daily: Vec<DailyTokens>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSnapshot {
    pub id: ProviderId,
    pub name: String,
    pub state: ProviderState,
    /// User-facing explanation for non-`ok` states.
    pub message: Option<String>,
    /// Plan label, e.g. "Max 20x", "Pro".
    pub plan: Option<String>,
    pub windows: Vec<UsageWindow>,
    pub credits: Option<Credits>,
    #[serde(default)]
    pub breakdown: Vec<BreakdownRow>,
    pub tokens: Option<TokenReport>,
    pub account_tokens: Option<AccountTokens>,
    /// Last successful plan-limit refresh.
    pub updated_at: Option<i64>,
}

impl ProviderSnapshot {
    /// An empty snapshot in the given state.
    pub fn empty(id: ProviderId, state: ProviderState) -> Self {
        ProviderSnapshot {
            id,
            name: id.display_name().to_string(),
            state,
            message: None,
            plan: None,
            windows: Vec::new(),
            credits: None,
            breakdown: Vec::new(),
            tokens: None,
            account_tokens: None,
            updated_at: None,
        }
    }

    /// The window with the highest usage, if any.
    pub fn most_constrained(&self) -> Option<&UsageWindow> {
        self.windows
            .iter()
            .max_by(|a, b| a.used_percent.total_cmp(&b.used_percent))
    }
}

/// One bar of the tray mini-meter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeterBar {
    pub provider: ProviderId,
    /// 0–100, or `None` when the provider has no data (draw an empty outline).
    pub percent: Option<f64>,
    pub level: Level,
    /// Window the bar represents.
    pub window_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Meter {
    /// One bar per provider in [`ProviderId::ALL`] order (disabled providers omitted).
    pub bars: Vec<MeterBar>,
    /// Worst level across bars.
    pub level: Level,
}

/// The whole state published to front ends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub schema_version: u32,
    pub revision: u64,
    pub generated_at: i64,
    pub meter: Meter,
    pub providers: Vec<ProviderSnapshot>,
}

impl Snapshot {
    /// Copy with every absolute timestamp moved by `delta` seconds.
    ///
    /// Fixture mode uses this so countdowns stay meaningful whenever a fixture is served.
    pub fn shifted(&self, delta: i64) -> Snapshot {
        let shift = |t: Option<i64>| t.map(|v| v + delta);
        let providers = self
            .providers
            .iter()
            .map(|p| ProviderSnapshot {
                windows: p
                    .windows
                    .iter()
                    .map(|w| UsageWindow {
                        resets_at: shift(w.resets_at),
                        observed_at: w.observed_at + delta,
                        ..w.clone()
                    })
                    .collect(),
                tokens: p.tokens.as_ref().map(|t| TokenReport {
                    updated_at: t.updated_at + delta,
                    ..t.clone()
                }),
                account_tokens: p.account_tokens.as_ref().map(|a| AccountTokens {
                    updated_at: a.updated_at + delta,
                    ..a.clone()
                }),
                updated_at: shift(p.updated_at),
                ..p.clone()
            })
            .collect();
        Snapshot {
            generated_at: self.generated_at + delta,
            providers,
            ..self.clone()
        }
    }

    /// JSON Schema of the snapshot contract.
    pub fn json_schema() -> schemars::Schema {
        schemars::schema_for!(Snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: &str, pct: f64) -> UsageWindow {
        UsageWindow {
            id: id.into(),
            label: id.into(),
            kind: WindowKind::Session,
            used_percent: pct,
            resets_at: Some(1_000),
            window_minutes: Some(300),
            level: Level::Normal,
            source: "oauth".into(),
            observed_at: 500,
        }
    }

    #[test]
    fn level_thresholds_are_inclusive() {
        assert_eq!(Level::from_percent(79.9, 80.0, 95.0), Level::Normal);
        assert_eq!(Level::from_percent(80.0, 80.0, 95.0), Level::Warning);
        assert_eq!(Level::from_percent(95.0, 80.0, 95.0), Level::Critical);
        assert!(Level::Critical > Level::Warning && Level::Warning > Level::Normal);
    }

    #[test]
    fn provider_id_round_trips() {
        for id in ProviderId::ALL {
            assert_eq!(id.as_str().parse::<ProviderId>().unwrap(), id);
            assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{id}\""));
        }
        assert!("gemini".parse::<ProviderId>().is_err());
    }

    #[test]
    fn most_constrained_picks_highest() {
        let mut p = ProviderSnapshot::empty(ProviderId::Claude, ProviderState::Ok);
        assert!(p.most_constrained().is_none());
        p.windows = vec![window("a", 10.0), window("b", 55.0), window("c", 30.0)];
        assert_eq!(p.most_constrained().unwrap().id, "b");
    }

    #[test]
    fn snapshot_serializes_camel_case() {
        let mut p = ProviderSnapshot::empty(ProviderId::Codex, ProviderState::Ok);
        p.windows = vec![window("codex:primary", 12.0)];
        p.tokens = Some(TokenReport {
            today: TokenTotals::default(),
            last7_days: TokenTotals::default(),
            by_model: vec![ModelTotals {
                model: "gpt-5.3-codex".into(),
                totals: TokenTotals {
                    total: 7,
                    ..TokenTotals::default()
                },
            }],
            updated_at: 1,
        });
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["windows"][0]["usedPercent"], 12.0);
        assert_eq!(json["windows"][0]["resetsAt"], 1_000);
        assert!(json["tokens"]["last7Days"].is_object());
        // flattened model totals
        assert_eq!(json["tokens"]["byModel"][0]["model"], "gpt-5.3-codex");
        assert_eq!(json["tokens"]["byModel"][0]["total"], 7);
        assert_eq!(json["state"], "ok");
        let back: ProviderSnapshot = serde_json::from_value(json).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn shifted_moves_every_timestamp() {
        let mut p = ProviderSnapshot::empty(ProviderId::Claude, ProviderState::Ok);
        p.windows = vec![window("session", 1.0)];
        p.updated_at = Some(400);
        p.account_tokens = Some(AccountTokens {
            today: 1,
            last7_days: 2,
            lifetime: None,
            daily: vec![],
            updated_at: 450,
        });
        let s = Snapshot {
            schema_version: SCHEMA_VERSION,
            revision: 3,
            generated_at: 600,
            meter: Meter {
                bars: vec![],
                level: Level::Normal,
            },
            providers: vec![p],
        };
        let t = s.shifted(100);
        assert_eq!(t.generated_at, 700);
        assert_eq!(t.providers[0].windows[0].resets_at, Some(1_100));
        assert_eq!(t.providers[0].windows[0].observed_at, 600);
        assert_eq!(t.providers[0].updated_at, Some(500));
        assert_eq!(
            t.providers[0].account_tokens.as_ref().unwrap().updated_at,
            550
        );
        // original untouched
        assert_eq!(s.generated_at, 600);
    }
}
