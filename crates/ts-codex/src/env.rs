//! The process environment [`CodexProvider`](crate::CodexProvider) reads from,
//! factored out so tests can substitute every external dependency: `$HOME`,
//! `$CODEX_HOME`, which `codex` binary runs, and which HTTP host the fallback
//! calls.

use std::path::PathBuf;

use ts_core::discovery::SearchEnv;

#[derive(Debug, Clone)]
pub struct CodexEnv {
    /// `$HOME`, used to expand `~` in configured homes and to default to `~/.codex`.
    pub home: PathBuf,
    /// `$CODEX_HOME`, if set.
    pub codex_home: Option<String>,
    /// Skip [`ts_core::discovery::find_binary_in`] and use this path directly.
    pub binary_override: Option<PathBuf>,
    /// Base URL for the HTTP fallback (`https://chatgpt.com` in production).
    pub http_base_url: String,
    /// Search environment ([`SearchEnv`]) used to discover the real `codex` binary.
    pub search: SearchEnv,
}

impl CodexEnv {
    /// The real process environment.
    pub fn current() -> CodexEnv {
        CodexEnv {
            home: ts_core::discovery::home_dir(),
            codex_home: std::env::var("CODEX_HOME").ok(),
            binary_override: None,
            http_base_url: "https://chatgpt.com".to_string(),
            search: SearchEnv::current(),
        }
    }
}
