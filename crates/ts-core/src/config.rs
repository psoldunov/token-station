//! Daemon configuration (`$XDG_CONFIG_HOME/token-station/config.toml`).
//!
//! The same structure travels as JSON over D-Bus (`GetSettings`/`SetSettings`), with
//! snake_case keys. Every field has a default, so partial files are fine.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Hard floors that protect the provider endpoints from aggressive polling.
pub const MIN_LIMITS_INTERVAL_SECS: u64 = 120;
pub const MIN_TOKENS_INTERVAL_SECS: u64 = 15;
pub const MIN_CLAUDE_ENDPOINT_INTERVAL_SECS: u64 = 120;

/// Ceiling on the plan-limit interval: a day, which is what every front end's
/// slider stops at. A hand-written `config.toml` may not go past it either, or the
/// next save from a front end would quietly clamp a value the user chose on
/// purpose and never say so.
pub const MAX_LIMITS_INTERVAL_SECS: u64 = 86_400;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub general: GeneralConfig,
    pub meter: MeterConfig,
    pub alerts: AlertsConfig,
    pub pricing: PricingConfig,
    pub claude: ClaudeConfig,
    pub codex: CodexConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeneralConfig {
    /// Plan-limit polling interval.
    pub limits_interval_secs: u64,
    /// Local log scan interval.
    pub tokens_interval_secs: u64,
    /// Days of limit history kept for the sparkline.
    pub history_retention_days: u32,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        GeneralConfig {
            limits_interval_secs: 300,
            tokens_interval_secs: 60,
            history_retention_days: 35,
        }
    }
}

/// Which window a tray meter bar shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeterWindow {
    #[default]
    MostConstrained,
    Session,
    Weekly,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MeterConfig {
    pub window: MeterWindow,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AlertsConfig {
    pub warning_percent: f64,
    pub critical_percent: f64,
    /// Desktop notifications when a window crosses warning/critical.
    pub notify: bool,
    /// Desktop notification when a window resets.
    pub notify_on_reset: bool,
}

impl Default for AlertsConfig {
    fn default() -> Self {
        AlertsConfig {
            warning_percent: 80.0,
            critical_percent: 95.0,
            notify: true,
            notify_on_reset: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PricingConfig {
    /// Refresh prices daily from `url` (the bundled table is always the fallback).
    pub auto_update: bool,
    pub url: String,
}

impl Default for PricingConfig {
    fn default() -> Self {
        PricingConfig {
            auto_update: true,
            url: "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json"
                .into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClaudeConfig {
    pub enabled: bool,
    /// Empty: `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
    pub config_dir: String,
    /// Empty: discover `claude` on PATH and well-known directories.
    pub binary: String,
    /// Poll `api.anthropic.com/api/oauth/usage` with the CLI's token (read-only).
    pub use_oauth_endpoint: bool,
    /// Empty: `claude-code/<installed version>`, matching the CLI's own `/usage` call.
    pub user_agent: String,
    pub min_endpoint_interval_secs: u64,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        ClaudeConfig {
            enabled: true,
            config_dir: String::new(),
            binary: String::new(),
            use_oauth_endpoint: true,
            user_agent: String::new(),
            min_endpoint_interval_secs: 180,
        }
    }
}

/// How the Codex app-server child is run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexProcessMode {
    /// Spawn per refresh, keep it `linger_secs`, then stop it (~150 MB saved when idle).
    #[default]
    OnDemand,
    /// Keep one child running.
    Persistent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CodexConfig {
    pub enabled: bool,
    /// Empty: discover `codex` on PATH and well-known directories.
    pub binary: String,
    /// Codex homes to read logs from. Empty: `$CODEX_HOME`, else `~/.codex`.
    pub homes: Vec<String>,
    pub process_mode: CodexProcessMode,
    pub linger_secs: u64,
    /// Fall back to the ChatGPT usage endpoint when app-server is unavailable.
    pub http_fallback: bool,
}

impl Default for CodexConfig {
    fn default() -> Self {
        CodexConfig {
            enabled: true,
            binary: String::new(),
            homes: Vec::new(),
            process_mode: CodexProcessMode::OnDemand,
            linger_secs: 60,
            http_fallback: true,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid TOML in {path}: {source}")]
    Toml {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("invalid settings JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("cannot serialize config: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("invalid settings: {}", .0.join("; "))]
    Invalid(Vec<String>),
}

impl Config {
    /// Load from `path`; a missing file yields defaults.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let cfg: Config = toml::from_str(&text).map_err(|source| ConfigError::Toml {
                    path: path.to_path_buf(),
                    source,
                })?;
                cfg.validated()
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(source) => Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Parse settings JSON as sent over D-Bus; missing keys take defaults.
    pub fn from_json(text: &str) -> Result<Config, ConfigError> {
        let cfg: Config = serde_json::from_str(text)?;
        cfg.validated()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }

    pub fn to_toml(&self) -> Result<String, ConfigError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Validate bounds; returns every problem at once.
    pub fn validated(self) -> Result<Config, ConfigError> {
        let problems = self.problems();
        if problems.is_empty() {
            Ok(self)
        } else {
            Err(ConfigError::Invalid(problems))
        }
    }

    fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let g = &self.general;
        if !(MIN_LIMITS_INTERVAL_SECS..=MAX_LIMITS_INTERVAL_SECS).contains(&g.limits_interval_secs)
        {
            out.push(format!(
                "general.limits_interval_secs must be \
                 {MIN_LIMITS_INTERVAL_SECS}..={MAX_LIMITS_INTERVAL_SECS}"
            ));
        }
        if g.tokens_interval_secs < MIN_TOKENS_INTERVAL_SECS {
            out.push(format!(
                "general.tokens_interval_secs must be >= {MIN_TOKENS_INTERVAL_SECS}"
            ));
        }
        if !(1..=366).contains(&g.history_retention_days) {
            out.push("general.history_retention_days must be 1..=366".into());
        }
        let a = &self.alerts;
        let pct = |v: f64| (1.0..=100.0).contains(&v);
        if !pct(a.warning_percent) || !pct(a.critical_percent) {
            out.push("alerts thresholds must be within 1..=100".into());
        }
        if a.warning_percent > a.critical_percent {
            out.push("alerts.warning_percent must not exceed alerts.critical_percent".into());
        }
        if self.claude.min_endpoint_interval_secs < MIN_CLAUDE_ENDPOINT_INTERVAL_SECS {
            out.push(format!(
                "claude.min_endpoint_interval_secs must be >= {MIN_CLAUDE_ENDPOINT_INTERVAL_SECS}"
            ));
        }
        if self.codex.linger_secs > 3600 {
            out.push("codex.linger_secs must be <= 3600".into());
        }
        if self.pricing.auto_update && !self.pricing.url.starts_with("https://") {
            out.push("pricing.url must be an https:// URL".into());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        assert!(Config::default().validated().is_ok());
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config::load(&dir.path().join("nope.toml")).unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn partial_toml_fills_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[alerts]\nwarning_percent = 70\n[codex]\nenabled = false\n",
        )
        .unwrap();
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.alerts.warning_percent, 70.0);
        assert_eq!(cfg.alerts.critical_percent, 95.0);
        assert!(!cfg.codex.enabled);
        assert!(cfg.claude.enabled);
    }

    #[test]
    fn toml_round_trip() {
        let cfg = Config::default();
        let text = cfg.to_toml().unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn json_round_trip_uses_snake_case() {
        let json = Config::default().to_json();
        assert!(json.contains("\"limits_interval_secs\":300"));
        assert!(json.contains("\"process_mode\":\"on_demand\""));
        assert_eq!(Config::from_json(&json).unwrap(), Config::default());
    }

    #[test]
    fn rejects_unknown_keys_and_bad_bounds() {
        assert!(Config::from_json(r#"{"general":{"bogus":1}}"#).is_err());
        let err = Config::from_json(
            r#"{"general":{"limits_interval_secs":5},"alerts":{"warning_percent":99,"critical_percent":90}}"#,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("limits_interval_secs"), "{msg}");
        assert!(msg.contains("must not exceed"), "{msg}");
    }

    #[test]
    fn the_limits_interval_is_bounded_at_both_ends() {
        let with = |secs: u64| {
            let mut cfg = Config::default();
            cfg.general.limits_interval_secs = secs;
            cfg.validated()
        };
        // Every front end's slider stops at a day; a longer hand-written value
        // would be silently clamped the next time one of them saves.
        assert!(with(MAX_LIMITS_INTERVAL_SECS).is_ok());
        assert!(with(MIN_LIMITS_INTERVAL_SECS).is_ok());
        for out_of_range in [MIN_LIMITS_INTERVAL_SECS - 1, MAX_LIMITS_INTERVAL_SECS + 1] {
            let msg = with(out_of_range).unwrap_err().to_string();
            assert!(msg.contains("limits_interval_secs"), "{msg}");
            assert!(msg.contains("86400"), "{msg}");
        }
    }

    #[test]
    fn invalid_toml_reports_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[general\n").unwrap();
        let err = Config::load(&path).unwrap_err();
        assert!(err.to_string().contains("config.toml"));
    }

    #[test]
    fn rejects_plain_http_pricing_url() {
        let err =
            Config::from_json(r#"{"pricing":{"url":"http://example.com/p.json"}}"#).unwrap_err();
        assert!(err.to_string().contains("https"));
    }
}
