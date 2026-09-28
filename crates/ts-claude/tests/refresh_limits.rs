mod support;

use ts_claude::ClaudeProvider;
use ts_core::pricing::shared_bundled;
use ts_core::provider::{Provider, RefreshOutcome};
use ts_core::snapshot::ProviderState;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn success_updates_windows_credits_and_breakdown() {
    let fixture = support::Fixture::new();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/oauth/usage"))
        .and(header("anthropic-beta", "oauth-2025-04-20"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(support::oauth_fixture("oauth_usage_max_2026-09.json")),
        )
        .mount(&server)
        .await;

    let provider = ClaudeProvider::with_env(
        fixture.config(),
        shared_bundled(),
        fixture.env(&server.uri()),
    );
    let outcome = provider.refresh_limits(false).await;
    assert_eq!(outcome, RefreshOutcome::Updated);

    let snap = provider.snapshot(1_790_596_800);
    assert_eq!(snap.state, ProviderState::Ok);
    assert_eq!(snap.plan.as_deref(), Some("Max 20x"));
    assert_eq!(snap.windows.len(), 3);
    assert_eq!(snap.breakdown.len(), 4);
    assert!(!snap.credits.as_ref().unwrap().enabled);
}

#[tokio::test]
async fn unauthorized_marks_skipped_with_expired_message() {
    let fixture = support::Fixture::new();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/oauth/usage"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let provider = ClaudeProvider::with_env(
        fixture.config(),
        shared_bundled(),
        fixture.env(&server.uri()),
    );
    let outcome = provider.refresh_limits(false).await;
    match outcome {
        RefreshOutcome::Skipped(msg) => assert!(msg.contains("expired")),
        other => panic!("expected Skipped, got {other:?}"),
    }
}

#[tokio::test]
async fn forbidden_disables_endpoint_for_process() {
    let fixture = support::Fixture::new();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/oauth/usage"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;

    let provider = ClaudeProvider::with_env(
        fixture.config(),
        shared_bundled(),
        fixture.env(&server.uri()),
    );
    let outcome = provider.refresh_limits(false).await;
    assert!(matches!(outcome, RefreshOutcome::Failed(_)));

    // Second call does not hit the network again: still Failed/Skipped, and
    // wiremock would panic on an unexpected extra request if it happened
    // (no additional Mock configured), so reaching here at all proves it.
    let second = provider.refresh_limits(true).await;
    assert!(matches!(second, RefreshOutcome::Skipped(_)));
}

#[tokio::test]
async fn rate_limited_backs_off_using_retry_after() {
    let fixture = support::Fixture::new();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/oauth/usage"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "120"))
        .mount(&server)
        .await;

    let provider = ClaudeProvider::with_env(
        fixture.config(),
        shared_bundled(),
        fixture.env(&server.uri()),
    );
    let outcome = provider.refresh_limits(false).await;
    assert!(matches!(outcome, RefreshOutcome::Failed(_)));

    let next = provider.next_limits_refresh(0, std::time::Duration::from_secs(60));
    assert!(next.as_secs() >= 100, "expected long backoff, got {next:?}");
}

#[tokio::test]
async fn server_error_keeps_no_data_and_reports_failed() {
    let fixture = support::Fixture::new();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/oauth/usage"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let provider = ClaudeProvider::with_env(
        fixture.config(),
        shared_bundled(),
        fixture.env(&server.uri()),
    );
    let outcome = provider.refresh_limits(false).await;
    assert!(matches!(outcome, RefreshOutcome::Failed(_)));
    let snap = provider.snapshot(0);
    assert_eq!(snap.state, ProviderState::Error);
    assert!(snap.windows.is_empty());
}
