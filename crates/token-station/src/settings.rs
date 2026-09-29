//! Config changes: what a new [`Config`] makes the daemon redo.

use std::path::Path;

use ts_core::config::{Config, ConfigError};

use crate::atomic::{Unwritable, check_replaceable, write_atomic};

/// What `SetSettings` reports when `config.toml` is not ours to rewrite.
pub const MANAGED_DECLARATIVELY: &str =
    "config.toml is managed declaratively (e.g. by Nix); change it there";

/// Why [`persist`] did not write the file.
///
/// The two are kept apart because the caller can act on them differently: a
/// managed file is something the user has to change elsewhere, while a failed
/// write is the daemon's problem and the front end can only report it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PersistError {
    /// Somebody else owns `config.toml`; nothing was written.
    #[error("{MANAGED_DECLARATIVELY}")]
    Managed,
    /// The file could not be written — a full disk, a read-only mount, a
    /// directory in the way.
    #[error("{0}")]
    Write(String),
}

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
///
/// A symlink or a read-only file is somebody else's to change: renaming over it
/// would replace a `/nix/store` link and the next rebuild would undo the change
/// anyway, so it is refused instead.
pub fn persist(config: &Config, path: &Path) -> Result<(), PersistError> {
    if let Err(Unwritable::Symlink | Unwritable::ReadOnly) = check_replaceable(path) {
        return Err(PersistError::Managed);
    }
    let text = config
        .to_toml()
        .map_err(|error| PersistError::Write(error.to_string()))?;
    write_atomic(path, text.as_bytes())
        .map_err(|error| PersistError::Write(format!("cannot write {}: {error}", path.display())))
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
    fn a_config_somebody_else_manages_is_refused_rather_than_replaced() {
        use std::os::unix::fs::PermissionsExt;
        const MANAGED: &str = "# managed\n";
        let dir = tempfile::tempdir().unwrap();

        // What home-manager leaves behind: a symlink into the store.
        let target = dir.path().join("store-config.toml");
        std::fs::write(&target, MANAGED).unwrap();
        let link = dir.path().join("linked.toml");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        // And what a plain read-only copy looks like.
        let read_only = dir.path().join("read-only.toml");
        std::fs::write(&read_only, MANAGED).unwrap();
        std::fs::set_permissions(&read_only, std::fs::Permissions::from_mode(0o444)).unwrap();

        for path in [&link, &read_only] {
            assert_eq!(
                persist(&Config::default(), path).unwrap_err(),
                PersistError::Managed,
                "{}",
                path.display()
            );
            assert_eq!(std::fs::read_to_string(path).unwrap(), MANAGED);
        }
        assert!(
            std::fs::symlink_metadata(&link).unwrap().is_symlink(),
            "the link itself was not replaced either"
        );
    }

    #[test]
    fn a_write_failure_is_reported_rather_than_swallowed() {
        let dir = tempfile::tempdir().unwrap();
        // A directory where the file should be: the rename cannot succeed.
        let path = dir.path().join("config.toml");
        std::fs::create_dir(&path).unwrap();
        let error = persist(&Config::default(), &path).unwrap_err();
        // A failed write is the daemon's problem, not bad input, and it is told
        // apart from a file somebody else manages.
        assert!(matches!(error, PersistError::Write(_)), "{error:?}");
        assert!(error.to_string().contains("cannot write"), "{error}");
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
