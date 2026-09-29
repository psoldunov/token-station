//! Hits the real `api.anthropic.com` oauth/usage endpoint with the real local
//! `claude` credentials. Opt-in only: `TS_LIVE=1 cargo test -p ts-claude --test live -- --ignored`.
//! Never prints the access token or any response body.

use ts_claude::ClaudeProvider;
use ts_core::pricing::shared_bundled;
use ts_core::provider::{Provider, RefreshOutcome};

#[tokio::test]
#[ignore = "hits the real api.anthropic.com endpoint; opt in with TS_LIVE=1"]
async fn live_oauth_usage_call() {
    if std::env::var("TS_LIVE").as_deref() != Ok("1") {
        eprintln!("skipping: set TS_LIVE=1 to run against the real Claude endpoint");
        return;
    }
    let config = ts_core::config::ClaudeConfig::default();
    let provider = ClaudeProvider::new(config, shared_bundled());
    let outcome = provider.refresh_limits(false).await;
    match outcome {
        RefreshOutcome::Updated => {
            let snap = provider.snapshot(0);
            eprintln!(
                "live refresh ok: {} windows, plan={:?}",
                snap.windows.len(),
                snap.plan
            );
        }
        other => panic!("live refresh did not succeed: {other:?}"),
    }
}
