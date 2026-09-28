//! Where the daemon keeps its files.
//!
//! Every location follows the XDG base directory spec. The environment is taken as
//! an explicit value so tests can build a [`Paths`] pointing into a temp directory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Directory name used under every XDG root.
pub const APP_DIR: &str = "token-station";

/// The subset of the environment that decides the paths.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Env {
    pub home: Option<String>,
    pub config_home: Option<String>,
    pub state_home: Option<String>,
    pub cache_home: Option<String>,
    pub runtime_dir: Option<String>,
    /// `$XDG_DATA_HOME`; where `setup` drops applets, extensions and icons.
    pub data_home: Option<String>,
}

impl Env {
    /// Read the relevant variables from the process environment.
    pub fn current() -> Env {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Env {
            home: var("HOME"),
            config_home: var("XDG_CONFIG_HOME"),
            state_home: var("XDG_STATE_HOME"),
            cache_home: var("XDG_CACHE_HOME"),
            runtime_dir: var("XDG_RUNTIME_DIR"),
            data_home: var("XDG_DATA_HOME"),
        }
    }

    /// Build from a map, for tests.
    pub fn from_map(vars: &HashMap<&str, &str>) -> Env {
        let var = |k: &str| vars.get(k).map(|v| (*v).to_string());
        Env {
            home: var("HOME"),
            config_home: var("XDG_CONFIG_HOME"),
            state_home: var("XDG_STATE_HOME"),
            cache_home: var("XDG_CACHE_HOME"),
            runtime_dir: var("XDG_RUNTIME_DIR"),
            data_home: var("XDG_DATA_HOME"),
        }
    }

    /// `$HOME`, or `/tmp` when the environment has none.
    pub fn home_dir(&self) -> PathBuf {
        home_of(self)
    }

    /// `$XDG_DATA_HOME`, defaulting to `~/.local/share`.
    pub fn data_home(&self) -> PathBuf {
        root(self.data_home.as_ref(), &home_of(self), ".local/share")
    }

    /// `$XDG_CONFIG_HOME`, defaulting to `~/.config`.
    pub fn config_home(&self) -> PathBuf {
        root(self.config_home.as_ref(), &home_of(self), ".config")
    }

    /// `$XDG_STATE_HOME`, defaulting to `~/.local/state`.
    pub fn state_home(&self) -> PathBuf {
        root(self.state_home.as_ref(), &home_of(self), ".local/state")
    }
}

/// Resolved file locations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config_file: PathBuf,
    pub state_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub runtime_dir: PathBuf,
}

fn home_of(env: &Env) -> PathBuf {
    env.home
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn root(explicit: Option<&String>, home: &Path, fallback: &str) -> PathBuf {
    match explicit {
        Some(value) => PathBuf::from(value),
        None => home.join(fallback),
    }
}

impl Paths {
    /// Resolve every location from `env`.
    pub fn resolve(env: &Env) -> Paths {
        let home = home_of(env);
        let config = root(env.config_home.as_ref(), &home, ".config").join(APP_DIR);
        let state = root(env.state_home.as_ref(), &home, ".local/state").join(APP_DIR);
        let cache = root(env.cache_home.as_ref(), &home, ".cache").join(APP_DIR);
        // No runtime dir (cron, ssh without pam_systemd): keep the file with the state.
        let runtime = match env.runtime_dir.as_ref() {
            Some(value) => PathBuf::from(value).join(APP_DIR),
            None => state.join("run"),
        };
        Paths {
            config_file: config.join("config.toml"),
            state_dir: state,
            cache_dir: cache,
            runtime_dir: runtime,
        }
    }

    /// Paths for the current process environment.
    pub fn current() -> Paths {
        Paths::resolve(&Env::current())
    }

    pub fn history_db(&self) -> PathBuf {
        self.state_dir.join("history.sqlite")
    }

    pub fn alerts_file(&self) -> PathBuf {
        self.state_dir.join("alerts.json")
    }

    pub fn pricing_cache(&self) -> PathBuf {
        self.cache_dir.join("pricing.json")
    }

    /// Drop box used by `token-station statusline` when the daemon is not reachable.
    pub fn statusline_drop(&self) -> PathBuf {
        self.runtime_dir.join("claude-statusline.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Env {
        Env::from_map(&pairs.iter().copied().collect())
    }

    #[test]
    fn xdg_variables_win() {
        let p = Paths::resolve(&env(&[
            ("HOME", "/home/u"),
            ("XDG_CONFIG_HOME", "/cfg"),
            ("XDG_STATE_HOME", "/st"),
            ("XDG_CACHE_HOME", "/ca"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ]));
        assert_eq!(p.config_file, Path::new("/cfg/token-station/config.toml"));
        assert_eq!(
            p.history_db(),
            Path::new("/st/token-station/history.sqlite")
        );
        assert_eq!(p.alerts_file(), Path::new("/st/token-station/alerts.json"));
        assert_eq!(
            p.pricing_cache(),
            Path::new("/ca/token-station/pricing.json")
        );
        assert_eq!(
            p.statusline_drop(),
            Path::new("/run/user/1000/token-station/claude-statusline.json")
        );
    }

    #[test]
    fn falls_back_to_home_defaults() {
        let p = Paths::resolve(&env(&[("HOME", "/home/u")]));
        assert_eq!(
            p.config_file,
            Path::new("/home/u/.config/token-station/config.toml")
        );
        assert_eq!(
            p.history_db(),
            Path::new("/home/u/.local/state/token-station/history.sqlite")
        );
        assert_eq!(
            p.pricing_cache(),
            Path::new("/home/u/.cache/token-station/pricing.json")
        );
        // Without XDG_RUNTIME_DIR the drop box lives next to the state.
        assert_eq!(
            p.statusline_drop(),
            Path::new("/home/u/.local/state/token-station/run/claude-statusline.json")
        );
    }

    #[test]
    fn xdg_roots_have_spec_defaults() {
        let explicit = env(&[
            ("HOME", "/home/u"),
            ("XDG_DATA_HOME", "/da"),
            ("XDG_CONFIG_HOME", "/cfg"),
            ("XDG_STATE_HOME", "/st"),
        ]);
        assert_eq!(explicit.data_home(), Path::new("/da"));
        assert_eq!(explicit.config_home(), Path::new("/cfg"));
        assert_eq!(explicit.state_home(), Path::new("/st"));

        let bare = env(&[("HOME", "/home/u")]);
        assert_eq!(bare.data_home(), Path::new("/home/u/.local/share"));
        assert_eq!(bare.config_home(), Path::new("/home/u/.config"));
        assert_eq!(bare.state_home(), Path::new("/home/u/.local/state"));
        assert_eq!(bare.home_dir(), Path::new("/home/u"));
    }

    #[test]
    fn empty_home_still_resolves() {
        let p = Paths::resolve(&Env::default());
        assert!(p.config_file.starts_with("/tmp"));
        assert!(p.state_dir.ends_with("token-station"));
    }
}
