//! What the provider hands the daemon to keep across a restart, and what it
//! makes of it on the way back in.

mod support;

use serde_json::json;
use ts_claude::ClaudeProvider;
use ts_core::pricing::shared_bundled;
use ts_core::provider::{Provider, RefreshOutcome};
use ts_core::snapshot::ProviderState;
use wiremock::{MockServer, ResponseTemplate};

use support::mock_oauth_usage;

const WEEK_SECS: i64 = 7 * 86_400;

fn unix_now() -> i64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    i64::try_from(secs).unwrap()
}

fn provider_for(server: &MockServer, fixture: &support::Fixture) -> ClaudeProvider {
    ClaudeProvider::with_env(
        fixture.config(),
        shared_bundled(),
        fixture.env(&server.uri()),
    )
}

fn usage_ok() -> ResponseTemplate {
    ResponseTemplate::new(200)
        .set_body_string(support::oauth_fixture("oauth_usage_max_2026-09.json"))
}

async fn requests(server: &MockServer) -> usize {
    server.received_requests().await.map_or(0, |r| r.len())
}

fn rate_limited() -> ResponseTemplate {
    ResponseTemplate::new(429).insert_header("retry-after", "900")
}

/// What a provider on the fixture's Max sign-in would save once the endpoint
/// has answered it with `response`.
async fn saved_after(
    response: ResponseTemplate,
) -> (serde_json::Value, MockServer, support::Fixture) {
    let server = mock_oauth_usage(response).await;
    let fixture = support::Fixture::new();
    let first = provider_for(&server, &fixture);
    first.refresh_limits(false).await;
    let saved = first.saved_state().expect("an answer is worth saving");
    (saved, server, fixture)
}

/// A provider started afresh over `fixture` and handed `saved` back at `now`.
fn restarted_with(
    saved: serde_json::Value,
    server: &MockServer,
    fixture: &support::Fixture,
    now: i64,
) -> ClaudeProvider {
    let provider = provider_for(server, fixture);
    provider.restore_state(saved, now);
    provider
}

#[tokio::test]
async fn a_provider_that_has_done_nothing_saves_nothing() {
    let server = mock_oauth_usage(usage_ok()).await;
    let fixture = support::Fixture::new();
    assert_eq!(provider_for(&server, &fixture).saved_state(), None);
}

#[tokio::test]
async fn a_reading_survives_a_restart() {
    let (saved, server, fixture) = saved_after(usage_ok()).await;

    let now = unix_now();
    let restarted = restarted_with(saved, &server, &fixture, now);
    let snap = restarted.snapshot(now);
    assert_eq!(snap.state, ProviderState::Ok);
    assert_eq!(snap.message, None);
    assert_eq!(snap.plan.as_deref(), Some("Max 20x"));
    assert_eq!(snap.windows.len(), 3);
    assert_eq!(snap.breakdown.len(), 4);
    assert!(snap.updated_at.is_some());

    // The reading is seconds old, so the restarted provider keeps to the
    // endpoint floor rather than asking again straight away.
    assert!(matches!(
        restarted.refresh_limits(false).await,
        RefreshOutcome::Skipped(_)
    ));
    assert_eq!(requests(&server).await, 1);
}

#[tokio::test]
async fn a_backoff_survives_a_restart() {
    let (saved, server, fixture) = saved_after(rate_limited()).await;

    let now = unix_now();
    let restarted = restarted_with(saved, &server, &fixture, now);
    // Not even a forced refresh gets past a backoff carried over a restart.
    assert!(matches!(
        restarted.refresh_limits(true).await,
        RefreshOutcome::Skipped(_)
    ));
    assert_eq!(requests(&server).await, 1);
    let snap = restarted.snapshot(now);
    assert_eq!(snap.state, ProviderState::Loading);
    assert_eq!(
        snap.message.as_deref(),
        Some("Rate limited by Claude's usage endpoint; retrying automatically.")
    );
    let wait = restarted.next_limits_refresh(now, std::time::Duration::from_secs(60));
    assert!(
        wait.as_secs() >= 800,
        "expected the backoff to hold, got {wait:?}"
    );
}

fn sign_in_on_pro(fixture: &support::Fixture) {
    let pro = support::VALID_CREDENTIALS
        .replace(r#""max""#, r#""pro""#)
        .replace("default_claude_max_20x", "default_claude_pro");
    std::fs::write(fixture.credentials_path(), pro).unwrap();
}

#[tokio::test]
async fn a_backoff_saved_for_another_plan_is_not_restored() {
    let (saved, server, fixture) = saved_after(rate_limited()).await;
    sign_in_on_pro(&fixture);

    let restarted = restarted_with(saved, &server, &fixture, unix_now());
    assert!(matches!(
        restarted.refresh_limits(false).await,
        RefreshOutcome::Failed(_)
    ));
    assert_eq!(requests(&server).await, 2, "the old plan's backoff held");
}

#[tokio::test]
async fn a_sign_in_on_another_plan_drops_the_restored_backoff() {
    let (saved, server, fixture) = saved_after(rate_limited()).await;
    // Nothing to read when the provider is built, so the restore is taken
    // at its word, until the sign-in turns up on another plan.
    std::fs::remove_file(fixture.credentials_path()).unwrap();
    let restarted = restarted_with(saved, &server, &fixture, unix_now());
    sign_in_on_pro(&fixture);

    assert!(matches!(
        restarted.refresh_limits(false).await,
        RefreshOutcome::Failed(_)
    ));
    assert_eq!(requests(&server).await, 2, "the old plan's backoff held");
}

#[tokio::test]
async fn a_reading_older_than_a_week_is_dropped() {
    let (saved, server, fixture) = saved_after(usage_ok()).await;

    let later = unix_now() + WEEK_SECS + 60;
    let restarted = restarted_with(saved, &server, &fixture, later);
    let snap = restarted.snapshot(later);
    assert!(snap.windows.is_empty());
    assert_eq!(snap.plan, None);
    assert_eq!(snap.updated_at, None);
}

#[tokio::test]
async fn timestamps_from_the_future_cannot_wedge_refreshes() {
    let server = mock_oauth_usage(usage_ok()).await;
    let fixture = support::Fixture::new();
    let provider = provider_for(&server, &fixture);
    let now = unix_now();
    // A clock that has since been set back: taken at face value, this
    // attempt would hold off every refresh for a day.
    provider.restore_state(
        json!({
            "version": 1,
            "lastAttemptAt": now + 86_400,
            "lastEndpointSuccessAt": now + 86_400,
        }),
        now - 600,
    );
    assert_eq!(
        provider.refresh_limits(false).await,
        RefreshOutcome::Updated
    );
    assert_eq!(requests(&server).await, 1);
}

#[tokio::test]
async fn a_backoff_past_any_retry_after_is_cut_to_a_day() {
    let server = mock_oauth_usage(usage_ok()).await;
    let fixture = support::Fixture::new();
    let provider = provider_for(&server, &fixture);
    let now = unix_now();
    provider.restore_state(
        json!({"version": 1, "backoffUntil": now + 30 * 86_400, "backoffCurrentSecs": 600}),
        now,
    );
    let wait = provider.next_limits_refresh(now, std::time::Duration::from_secs(60));
    assert!(
        wait.as_secs() <= 86_400,
        "expected at most a day, got {wait:?}"
    );
}

#[tokio::test]
async fn unreadable_saved_state_is_ignored() {
    let server = mock_oauth_usage(usage_ok()).await;
    let fixture = support::Fixture::new();
    let now = unix_now();
    for junk in [
        json!("not an object"),
        json!({"version": 99, "lastAttemptAt": now}),
        json!({"version": 1, "endpointWindows": "not a list"}),
    ] {
        let provider = provider_for(&server, &fixture);
        provider.restore_state(junk.clone(), now);
        assert_eq!(
            provider.snapshot(now).state,
            ProviderState::Loading,
            "{junk}"
        );
        assert_eq!(
            provider.refresh_limits(false).await,
            RefreshOutcome::Updated,
            "{junk}"
        );
    }
}
