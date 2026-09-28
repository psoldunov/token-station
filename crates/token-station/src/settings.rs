//! Config changes: what a new [`Config`] makes the daemon redo.

use std::path::Path;

use ts_core::config::{Config, ConfigError};

use crate::atomic::write_atomic;

/// Which parts of the daemon a config replacement invalidates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConfigChange {
    /// The Claude provider must be rebuilt.
    pub claude: bool,
    /// The Codex provider must be rebuilt.
    pub codex: bool,
    /// Polling intervals or retention changed.
    pub general: bool,
    /// Thresholds or notification switches changed.
    pub alerts: bool,
    /// Pricing source changed.
    pub pricing: bool,
    /// Which window the tray meter shows changed.
    pub meter: bool,
}

impl ConfigChange {
    /// Nothing at all changed.
    pub fn is_empty(self) -> bool {
        self == ConfigChange::default()
    }

    /// A new snapshot has to be published.
    pub fn needs_republish(self) -> bool {
        self.claude || self.codex || self.alerts || self.meter
    }
}

/// Compare two configs section by section.
pub fn diff(old: &Config, new: &Config) -> ConfigChange {
    ConfigChange {
        claude: old.claude != new.claude,
        codex: old.codex != new.codex,
        general: old.general != new.general,
        alerts: old.alerts != new.alerts,
        pricing: old.pricing != new.pricing,
        meter: old.meter != new.meter,
    }
}

/// Parse settings JSON as `SetSettings` receives it.
///
/// Returns every problem at once, so the D-Bus error can list them all.
pub fn parse_settings(json: &str) -> Result<Config, Vec<String>> {
    Config::from_json(json).map_err(|error| match error {
        ConfigError::Invalid(problems) => problems,
        other => vec![other.to_string()],
    })
}

/// Write `config` to `path` as TOML, atomically.
pub fn persist(config: &Config, path: &Path) -> Result<(), String> {
    let text = config.to_toml().map_err(|e| e.to_string())?;
    write_atomic(path, text.as_bytes()).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Re-read `path`, keeping `current` when the file is invalid.
pub fn reload(path: &Path, current: &Config) -> Config {
    match Config::load(path) {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(%error, "config reload failed, keeping the previous settings");
            current.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts_core::config::MeterWindow;

    #[test]
    fn identical_configs_change_nothing() {
        let change = diff(&Config::default(), &Config::default());
        assert!(change.is_empty());
        assert!(!change.needs_republish());
    }

    #[test]
    fn each_section_is_tracked_on_its_own() {
        let mut new = Config::default();
        new.claude.binary = "/usr/bin/claude".into();
        let change = diff(&Config::default(), &new);
        assert!(change.claude);
        assert!(!change.codex && !change.general && !change.alerts);
        assert!(change.needs_republish());

        let mut new = Config::default();
        new.codex.linger_secs = 30;
        assert!(diff(&Config::default(), &new).codex);

        let mut new = Config::default();
        new.general.tokens_interval_secs = 90;
        let change = diff(&Config::default(), &new);
        assert!(change.general && !change.needs_republish());

        let mut new = Config::default();
        new.meter.window = MeterWindow::Weekly;
        assert!(diff(&Config::default(), &new).needs_republish());

        let mut new = Config::default();
        new.pricing.auto_update = false;
        assert!(diff(&Config::default(), &new).pricing);
    }

    #[test]
    fn parse_settings_lists_every_problem() {
        let problems = parse_settings(
            r#"{"general":{"limits_interval_secs":5},"alerts":{"warning_percent":99,"critical_percent":90}}"#,
        )
        .unwrap_err();
        assert_eq!(problems.len(), 2);
        assert!(problems.iter().any(|p| p.contains("limits_interval_secs")));
    }

    #[test]
    fn parse_settings_reports_bad_json_once() {
        let problems = parse_settings("not json").unwrap_err();
        assert_eq!(problems.len(), 1);
        assert!(parse_settings("{}").is_ok());
    }

    #[test]
    fn persist_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg/config.toml");
        let mut config = Config::default();
        config.alerts.warning_percent = 70.0;
        persist(&config, &path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), config);
    }

    #[test]
    fn reload_keeps_the_old_config_when_the_file_is_broken() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[general\n").unwrap();
        let mut current = Config::default();
        current.alerts.warning_percent = 60.0;
        assert_eq!(reload(&path, &current), current);

        std::fs::write(&path, "[alerts]\nwarning_percent = 75\n").unwrap();
        assert_eq!(reload(&path, &current).alerts.warning_percent, 75.0);
    }
}
