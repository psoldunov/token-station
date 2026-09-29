use std::path::{Path, PathBuf};

use ts_claude::ClaudeEnv;
use ts_core::config::ClaudeConfig;
use ts_core::discovery::SearchEnv;

/// A tempdir laid out as a Claude config dir, with a valid (far-future
/// expiry) credentials file so tests don't race the real wall clock.
#[allow(
    dead_code,
    reason = "compiled separately into every integration test binary; not every binary uses every item"
)]
pub struct Fixture {
    pub dir: tempfile::TempDir,
}

#[allow(
    dead_code,
    reason = "compiled separately into every integration test binary; not every binary uses every item"
)]
impl Fixture {
    pub fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".credentials.json"), VALID_CREDENTIALS).unwrap();
        Fixture { dir }
    }

    pub fn config_dir(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }

    pub fn credentials_path(&self) -> PathBuf {
        self.config_dir().join(".credentials.json")
    }

    pub fn env(&self, api_base_url: &str) -> ClaudeEnv {
        ClaudeEnv {
            home: self.dir.path().to_path_buf(),
            claude_config_dir: None,
            api_base_url: api_base_url.to_string(),
            claude_binary: None,
            search: SearchEnv {
                path: None,
                home: self.dir.path().to_path_buf(),
                user: None,
            },
        }
    }

    pub fn config(&self) -> ClaudeConfig {
        ClaudeConfig {
            config_dir: self.config_dir().display().to_string(),
            user_agent: "claude-code/test".to_string(),
            min_endpoint_interval_secs: 180,
            ..Default::default()
        }
    }
}

pub const VALID_CREDENTIALS: &str = r#"{
  "claudeAiOauth": {
    "accessToken": "sk-ant-oat01-TESTTOKEN",
    "refreshToken": "sk-ant-ort01-TESTTOKEN",
    "expiresAt": 4102444800000,
    "scopes": ["user:profile"],
    "subscriptionType": "max",
    "rateLimitTier": "default_claude_max_20x"
  }
}"#;

#[allow(
    dead_code,
    reason = "compiled separately into every integration test binary; not every binary uses every item"
)]
pub fn oauth_fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}
