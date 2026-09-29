mod support;

use std::path::Path;

use ts_claude::ClaudeProvider;
use ts_core::pricing::shared_bundled;
use ts_core::provider::Provider;
use wiremock::matchers::{method, path as path_matcher};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn real_fixture_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/credentials.json")
}

#[tokio::test]
async fn provider_never_writes_or_touches_credentials_file() {
    let dir = tempfile::tempdir().unwrap();
    let creds_path = dir.path().join(".credentials.json");
    std::fs::copy(real_fixture_path(), &creds_path).unwrap();

    let before_bytes = std::fs::read(&creds_path).unwrap();
    let before_mtime = std::fs::metadata(&creds_path).unwrap().modified().unwrap();

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_matcher("/api/oauth/usage"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(support::oauth_fixture("oauth_usage_max_2026-09.json")),
        )
        .mount(&server)
        .await;

    let fixture = support::Fixture::new();
    let env = fixture.env(&server.uri());
    let mut config = fixture.config();
    config.config_dir = dir.path().display().to_string();
    let provider = ClaudeProvider::with_env(config, shared_bundled(), env);

    // Exercises the real (possibly-expired-by-wall-clock) fixture through
    // every code path that touches the credentials file, regardless of
    // whether it happens to be expired right now.
    let _ = provider.refresh_limits(true).await;
    provider.refresh_tokens().await;
    let _ = provider.snapshot(0);

    let after_bytes = std::fs::read(&creds_path).unwrap();
    let after_mtime = std::fs::metadata(&creds_path).unwrap().modified().unwrap();
    assert_eq!(
        before_bytes, after_bytes,
        "credentials file bytes must not change"
    );
    assert_eq!(
        before_mtime, after_mtime,
        "credentials file mtime must not change"
    );
}
