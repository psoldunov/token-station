mod support;

use ts_claude::ClaudeProvider;
use ts_core::pricing::shared_bundled;
use ts_core::provider::{Provider, RefreshOutcome};
use ts_core::snapshot::ProviderState;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A provider wired to `server`'s URI, using the shared test fixture defaults.
/// Returns the [`support::Fixture`] too: it owns the tempdir the provider
/// reads credentials from, so the caller must keep it alive for as long as
/// the provider is used.
fn provider_for(server: &MockServer) -> (ClaudeProvider, support::Fixture) {
    let fixture = support::Fixture::new();
    let provider = ClaudeProvider::with_env(
        fixture.config(),
        shared_bundled(),
        fixture.env(&server.uri()),
    );
    (provider, fixture)
}

/// A server that answers every `GET /api/oauth/usage` with `response`.
async fn mock_oauth_usage(response: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/oauth/usage"))
        .respond_with(response)
        .mount(&server)
        .await;
    server
}

/// Build a provider against `server` and run a single `refresh_limits(false)`.
async fn refresh_once(server: &MockServer) -> (RefreshOutcome, ClaudeProvider, support::Fixture) {
    let (provider, fixture) = provider_for(server);
    let outcome = provider.refresh_limits(false).await;
    (outcome, provider, fixture)
}

#[tokio::test]
async fn success_updates_windows_credits_and_breakdown() {
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

    let (provider, _fixture) = provider_for(&server);
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
    let server = mock_oauth_usage(ResponseTemplate::new(401)).await;
    let (provider, _fixture) = provider_for(&server);
    let outcome = provider.refresh_limits(false).await;
    match outcome {
        RefreshOutcome::Skipped(msg) => assert!(msg.contains("expired")),
        other => panic!("expected Skipped, got {other:?}"),
    }
}

/// What a failing status code should additionally do, beyond reporting
/// `RefreshOutcome::Failed`.
enum FailureFollowUp {
    /// A second call must not hit the network again: it should come back
    /// `Skipped`, and wiremock would panic on an unconfigured extra request
    /// if the endpoint were still being hit.
    SecondCallIsSkipped,
    /// The snapshot must keep no stale data and report `ProviderState::Error`.
    SnapshotHasNoDataAndErrors,
}

#[tokio::test]
async fn failing_status_codes_report_failed_and_follow_up_correctly() {
    let cases = [
        (403, FailureFollowUp::SecondCallIsSkipped),
        (500, FailureFollowUp::SnapshotHasNoDataAndErrors),
    ];
    for (status, follow_up) in cases {
        let server = mock_oauth_usage(ResponseTemplate::new(status)).await;
        let (outcome, provider, _fixture) = refresh_once(&server).await;
        assert!(
            matches!(outcome, RefreshOutcome::Failed(_)),
            "status {status}"
        );

        match follow_up {
            FailureFollowUp::SecondCallIsSkipped => {
                let second = provider.refresh_limits(true).await;
                assert!(
                    matches!(second, RefreshOutcome::Skipped(_)),
                    "status {status}"
                );
            }
            FailureFollowUp::SnapshotHasNoDataAndErrors => {
                let snap = provider.snapshot(0);
                assert_eq!(snap.state, ProviderState::Error, "status {status}");
                assert!(snap.windows.is_empty(), "status {status}");
            }
        }
    }
}

#[tokio::test]
async fn rate_limited_backs_off_using_retry_after() {
    let server =
        mock_oauth_usage(ResponseTemplate::new(429).insert_header("retry-after", "120")).await;
    let (provider, _fixture) = provider_for(&server);
    let outcome = provider.refresh_limits(false).await;
    assert!(matches!(outcome, RefreshOutcome::Failed(_)));

    let next = provider.next_limits_refresh(0, std::time::Duration::from_secs(60));
    assert!(next.as_secs() >= 100, "expected long backoff, got {next:?}");

    // Inside the backoff even a forced refresh stays off the network, and it
    // leaves the 429 as what the snapshot reports.
    let during_backoff = provider.refresh_limits(true).await;
    assert!(matches!(during_backoff, RefreshOutcome::Skipped(_)));
    let snap = provider.snapshot(1_790_596_800);
    assert_eq!(snap.state, ProviderState::Error);
    assert_eq!(
        snap.message.as_deref(),
        Some("Rate limited by Claude's usage endpoint.")
    );
    assert_eq!(server.received_requests().await.map(|r| r.len()), Some(1));
}
