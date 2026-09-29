mod support;

use ts_claude::ClaudeProvider;
use ts_core::pricing::shared_bundled;
use ts_core::provider::Provider;

#[tokio::test]
async fn refresh_tokens_populates_snapshot_token_report() {
    let fixture = support::Fixture::new();
    let projects_dir = fixture.config_dir().join("projects/some-proj");
    std::fs::create_dir_all(&projects_dir).unwrap();
    let line = r#"{"type":"assistant","timestamp":"2026-09-28T11:00:00Z","requestId":"req1","uuid":"u1","message":{"id":"msg1","model":"claude-opus-5-5","usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":10,"cache_creation_input_tokens":5}}}"#;
    std::fs::write(projects_dir.join("session.jsonl"), format!("{line}\n")).unwrap();

    let provider = ClaudeProvider::with_env(
        fixture.config(),
        shared_bundled(),
        fixture.env("https://unused.example"),
    );
    provider.refresh_tokens().await;

    let snap = provider.snapshot(1_790_600_000);
    let tokens = snap
        .tokens
        .expect("token report should be present after a scan");
    assert_eq!(tokens.last7_days.input, 100);
    assert_eq!(tokens.last7_days.output, 50);
    assert_eq!(tokens.last7_days.requests, 1);
    assert_eq!(tokens.by_model[0].model, "claude-opus-5-5");
}
