mod support;

use std::os::unix::fs::PermissionsExt;

use ts_claude::{ClaudeEnv, ClaudeProvider};
use ts_core::pricing::shared_bundled;
use ts_core::provider::{Provider, RefreshOutcome};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const EXPIRED_CREDENTIALS: &str = r#"{
  "claudeAiOauth": {
    "accessToken": "sk-ant-oat01-EXPIRED",
    "refreshToken": "sk-ant-ort01-EXPIRED",
    "expiresAt": 1,
    "scopes": ["user:profile"],
    "subscriptionType": "max",
    "rateLimitTier": "default_claude_max_20x"
  }
}"#;

fn write_fake_claude(
    dir: &std::path::Path,
    counter: &std::path::Path,
    fresh_creds: &std::path::Path,
    creds: &std::path::Path,
) -> std::path::PathBuf {
    let script_path = dir.join("fake-claude");
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  echo '2.1.283 (Claude Code)'\n  exit 0\nfi\nif [ \"$1\" = \"auth\" ]; then\n  printf x >> '{counter}'\n  cp '{fresh}' '{creds}'\n  echo '{{}}'\n  exit 0\nfi\nexit 1\n",
        counter = counter.display(),
        fresh = fresh_creds.display(),
        creds = creds.display(),
    );
    std::fs::write(&script_path, script).unwrap();
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    script_path
}

#[tokio::test]
async fn expired_credentials_are_refreshed_via_auth_status_at_most_once() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("claude");
    std::fs::create_dir_all(&config_dir).unwrap();
    let creds_path = config_dir.join(".credentials.json");
    std::fs::write(&creds_path, EXPIRED_CREDENTIALS).unwrap();

    let fresh_creds_path = dir.path().join("fresh-credentials.json");
    std::fs::write(&fresh_creds_path, support::VALID_CREDENTIALS).unwrap();

    let counter_path = dir.path().join("auth-status-calls");
    let script = write_fake_claude(dir.path(), &counter_path, &fresh_creds_path, &creds_path);

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/oauth/usage"))
        .and(header("user-agent", "claude-code/2.1.283"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(support::oauth_fixture("oauth_usage_max_2026-09.json")),
        )
        .mount(&server)
        .await;

    let config = ts_core::config::ClaudeConfig {
        config_dir: config_dir.display().to_string(),
        user_agent: String::new(), // force `claude --version` detection
        min_endpoint_interval_secs: 180,
        ..Default::default()
    };
    let env = ClaudeEnv {
        home: dir.path().to_path_buf(),
        claude_securestorage_config_dir: None,
        claude_config_dir: None,
        api_base_url: server.uri(),
        claude_binary: Some(script),
        search: ts_core::discovery::SearchEnv {
            // The real `PATH`, because the fake `claude` is a shell script that
            // runs `cp`, and the child's `PATH` is built from this one — as it
            // is in production, where `SearchEnv::current` always carries it.
            // Discovery itself is not using it: `claude_binary` names the
            // script outright.
            path: std::env::var("PATH").ok(),
            home: dir.path().to_path_buf(),
            user: None,
        },
    };
    let provider = ClaudeProvider::with_env(config, shared_bundled(), env);

    let first = provider.refresh_limits(false).await;
    assert_eq!(first, RefreshOutcome::Updated);
    assert_eq!(std::fs::read_to_string(&counter_path).unwrap().len(), 1);

    // The 10-minute auth-status throttle and the refresh soft/hard floors are
    // pure functions covered directly in state.rs unit tests; a second
    // real-time call here would need to wait out the hard 30s refresh floor.
}
