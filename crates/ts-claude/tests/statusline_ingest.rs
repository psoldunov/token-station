mod support;

use ts_claude::ClaudeProvider;
use ts_core::pricing::shared_bundled;
use ts_core::provider::{Ingest, IngestError, Provider};
use ts_core::snapshot::ProviderState;

#[tokio::test]
#[expect(
    clippy::float_cmp,
    reason = "compares exact literals that never went through arithmetic"
)]
async fn ingest_stores_session_and_weekly_and_marks_ok() {
    let fixture = support::Fixture::new();
    let provider = ClaudeProvider::with_env(
        fixture.config(),
        shared_bundled(),
        fixture.env("https://unused.example"),
    );

    let payload = support::oauth_fixture("statusline_input.json");
    provider
        .ingest(Ingest::ClaudeStatusline(payload), 1_790_596_900)
        .unwrap();

    let snap = provider.snapshot(1_790_596_900);
    assert_eq!(snap.state, ProviderState::Ok);
    let session = snap.windows.iter().find(|w| w.id == "session").unwrap();
    assert_eq!(session.used_percent, 34.0);
    assert_eq!(session.source, "statusline");
    let weekly = snap.windows.iter().find(|w| w.id == "weekly_all").unwrap();
    assert_eq!(weekly.used_percent, 15.0);
}

#[tokio::test]
async fn stale_statusline_does_not_force_ok() {
    let fixture = support::Fixture::new();
    let provider = ClaudeProvider::with_env(
        fixture.config(),
        shared_bundled(),
        fixture.env("https://unused.example"),
    );

    let payload = support::oauth_fixture("statusline_input.json");
    provider
        .ingest(Ingest::ClaudeStatusline(payload), 0)
        .unwrap();

    // 20 minutes later: statusline is no longer "fresh" (<10 min).
    let snap = provider.snapshot(1_200);
    assert_ne!(snap.state, ProviderState::Ok);
}

#[tokio::test]
async fn invalid_statusline_payload_is_rejected() {
    let fixture = support::Fixture::new();
    let provider = ClaudeProvider::with_env(
        fixture.config(),
        shared_bundled(),
        fixture.env("https://unused.example"),
    );
    let err = provider
        .ingest(Ingest::ClaudeStatusline("not json".to_string()), 0)
        .unwrap_err();
    assert!(matches!(err, IngestError::Invalid(_)));
}
