//! The process-environment inputs `ClaudeProvider` needs, separated out for tests.

use std::path::PathBuf;

use ts_core::discovery::SearchEnv;

/// Everything `ClaudeProvider` reads from the outside world besides `ClaudeConfig`.
#[derive(Debug, Clone)]
pub struct ClaudeEnv {
    pub home: PathBuf,
    /// Value of `$CLAUDE_CONFIG_DIR`, if set.
    pub claude_config_dir: Option<String>,
    /// Base URL of the OAuth usage endpoint, e.g. `https://api.anthropic.com`.
    pub api_base_url: String,
    /// Explicit `claude` binary path, bypassing discovery (tests only).
    pub claude_binary: Option<PathBuf>,
    /// Search environment used to discover the `claude` binary when
    /// `claude_binary` is `None`.
    pub search: SearchEnv,
}

impl ClaudeEnv {
    /// The real process environment (`HOME`, `CLAUDE_CONFIG_DIR`, `PATH`).
    #[must_use]
    pub fn current() -> ClaudeEnv {
        ClaudeEnv {
            home: SearchEnv::current().home,
            claude_config_dir: std::env::var("CLAUDE_CONFIG_DIR").ok(),
            api_base_url: "https://api.anthropic.com".to_string(),
            claude_binary: None,
            search: SearchEnv::current(),
        }
    }
}

/// Resolve the Claude config directory per `ClaudeConfig.config_dir` ||
/// `$CLAUDE_CONFIG_DIR` || `~/.claude`.
pub fn config_dir(config_dir_setting: &str, env: &ClaudeEnv) -> PathBuf {
    let trimmed = config_dir_setting.trim();
    if !trimmed.is_empty() {
        return ts_core::discovery::expand_tilde(trimmed, &env.home);
    }
    match env.claude_config_dir.as_deref().map(str::trim) {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => env.home.join(".claude"),
    }
}

/// Locate the `claude` binary: explicit test override first, then discovery.
pub fn find_claude_binary(binary_setting: &str, env: &ClaudeEnv) -> Option<PathBuf> {
    if let Some(path) = &env.claude_binary {
        return Some(path.clone());
    }
    ts_core::discovery::find_binary_in("claude", binary_setting, &env.search)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(home: &str, claude_config_dir: Option<&str>) -> ClaudeEnv {
        ClaudeEnv {
            home: PathBuf::from(home),
            claude_config_dir: claude_config_dir.map(str::to_string),
            api_base_url: "https://api.anthropic.com".to_string(),
            claude_binary: None,
            search: SearchEnv {
                path: None,
                home: PathBuf::from(home),
                user: None,
            },
        }
    }

    #[test]
    fn config_setting_wins_and_expands_tilde() {
        let e = env("/home/u", Some("/env/dir"));
        assert_eq!(config_dir("~/custom", &e), PathBuf::from("/home/u/custom"));
    }

    #[test]
    fn env_var_used_when_setting_empty() {
        let e = env("/home/u", Some("/env/dir"));
        assert_eq!(config_dir("", &e), PathBuf::from("/env/dir"));
    }

    #[test]
    fn falls_back_to_home_claude() {
        let e = env("/home/u", None);
        assert_eq!(config_dir("", &e), PathBuf::from("/home/u/.claude"));
    }

    #[test]
    fn empty_env_var_falls_back_to_home_claude() {
        let e = env("/home/u", Some("  "));
        assert_eq!(config_dir("", &e), PathBuf::from("/home/u/.claude"));
    }
}
