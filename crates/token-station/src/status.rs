//! `token-station status` and `token-station refresh`.
//!
//! With a daemon on the bus both commands are one D-Bus call. Without one, `status`
//! runs the providers once in-process so it still answers.

use std::sync::Arc;

use anyhow::Context;
use ts_core::assemble::assemble;
use ts_core::config::Config;
use ts_core::{ProviderState, Snapshot, TokenTotals};

use crate::clock::system_clock;
use crate::daemon::providers;
use crate::dbus::client;
use crate::notify::compact_duration;
use crate::paths::Paths;
use crate::pricing_refresh;

/// Human-readable status: one line per window, then the local token totals.
pub fn render(snapshot: &Snapshot, now: i64) -> String {
    let mut lines = Vec::new();
    for provider in &snapshot.providers {
        if provider.state != ProviderState::Ok && provider.state != ProviderState::Stale {
            lines.push(format!(
                "{} · {}",
                provider.name,
                state_text(provider.state, provider.message.as_deref())
            ));
            continue;
        }
        for window in &provider.windows {
            lines.push(format!(
                "{} · {} {}%{}",
                provider.name,
                window.label,
                window.used_percent.round() as i64,
                reset_suffix(window.resets_at, now),
            ));
        }
        if let Some(tokens) = &provider.tokens {
            lines.push(format!(
                "{} · today {} · 7 days {}",
                provider.name,
                totals_text(&tokens.today),
                totals_text(&tokens.last7_days),
            ));
        }
    }
    if lines.is_empty() {
        lines.push("No usage data yet".into());
    }
    lines.join("\n")
}

/// Human wording for a non-ok provider state, with its message appended.
pub fn state_text(state: ProviderState, message: Option<&str>) -> String {
    let label = match state {
        ProviderState::Loading => "loading",
        ProviderState::Unauthenticated => "not signed in",
        ProviderState::NotInstalled => "not installed",
        ProviderState::Disabled => "disabled",
        ProviderState::Error => "error",
        ProviderState::Ok => "ok",
        ProviderState::Stale => "stale",
    };
    match message {
        Some(text) if !text.is_empty() => format!("{label} — {text}"),
        _ => label.to_string(),
    }
}

/// ` · resets in 2h 14m`, or the empty string when the window never resets.
pub fn reset_suffix(resets_at: Option<i64>, now: i64) -> String {
    match resets_at.map(|t| t - now) {
        Some(remaining) if remaining > 0 => {
            format!(" · resets in {}", compact_duration(remaining))
        }
        _ => String::new(),
    }
}

/// `317.2M tokens · $228.17`.
pub fn totals_text(totals: &TokenTotals) -> String {
    let tokens = format_count(totals.total);
    match totals.cost_usd {
        Some(cost) => format!("{tokens} tokens · ${cost:.2}"),
        None => format!("{tokens} tokens"),
    }
}

/// `1.2M`, `34.5k`, `812`.
pub fn format_count(value: u64) -> String {
    match value {
        0..=9_999 => value.to_string(),
        10_000..=999_999 => format!("{:.1}k", value as f64 / 1_000.0),
        _ => format!("{:.1}M", value as f64 / 1_000_000.0),
    }
}

/// Snapshot from the daemon, or one produced locally when it is not running.
pub async fn current_snapshot(paths: &Paths) -> anyhow::Result<Snapshot> {
    if let Ok(connection) = zbus::Connection::session().await {
        if client::daemon_is_running(&connection).await {
            let proxy = client::connect(&connection).await?;
            let json = proxy.snapshot().await?;
            return serde_json::from_str(&json).context("the daemon sent an unreadable snapshot");
        }
    }
    Ok(run_providers_once(paths).await)
}

/// One-shot in-process refresh, for when no daemon is running.
async fn run_providers_once(paths: &Paths) -> Snapshot {
    let config = crate::daemon::run::load_config(&paths.config_file);
    let pricing = Arc::new(std::sync::RwLock::new(pricing_refresh::bundled_with_cache(
        &paths.pricing_cache(),
    )));
    let built = providers::build_all(&config, &pricing);
    let mut snapshots = Vec::new();
    for provider in built.values() {
        provider.refresh_limits(true).await;
        provider.refresh_tokens().await;
        snapshots.push(provider.snapshot(system_clock()()));
    }
    assemble(1, system_clock()(), &snapshots, &config)
}

/// `token-station status`.
pub async fn status(paths: &Paths, as_json: bool) -> anyhow::Result<()> {
    let snapshot = current_snapshot(paths).await?;
    let out = if as_json {
        serde_json::to_string_pretty(&snapshot)?
    } else {
        render(&snapshot, system_clock()())
    };
    println!("{out}");
    Ok(())
}

/// `token-station refresh`.
pub async fn refresh() -> anyhow::Result<()> {
    let connection = zbus::Connection::session()
        .await
        .context("cannot connect to the session bus")?;
    anyhow::ensure!(
        client::daemon_is_running(&connection).await,
        "no token-station daemon is running"
    );
    client::connect(&connection).await?.refresh().await?;
    Ok(())
}

/// Defaults used when no config file exists, exposed for tests.
pub fn default_config() -> Config {
    Config::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts_core::{Level, ProviderId, ProviderSnapshot, TokenReport, UsageWindow, WindowKind};

    fn window(label: &str, percent: f64, resets_at: Option<i64>) -> UsageWindow {
        UsageWindow {
            id: label.to_ascii_lowercase(),
            label: label.into(),
            kind: WindowKind::Session,
            used_percent: percent,
            resets_at,
            window_minutes: Some(300),
            level: Level::Normal,
            source: "oauth".into(),
            observed_at: 0,
        }
    }

    fn snapshot(providers: Vec<ProviderSnapshot>) -> Snapshot {
        assemble(1, 0, &providers, &Config::default())
    }

    #[test]
    fn one_line_per_window() {
        let mut claude = ProviderSnapshot::empty(ProviderId::Claude, ProviderState::Ok);
        claude.windows = vec![
            window("Session", 34.0, Some(8_040)),
            window("Weekly", 15.0, None),
        ];
        let text = render(&snapshot(vec![claude]), 0);
        assert_eq!(
            text,
            "Claude Code · Session 34% · resets in 2h 14m\nClaude Code · Weekly 15%"
        );
    }

    #[test]
    fn token_totals_follow_the_windows() {
        let mut claude = ProviderSnapshot::empty(ProviderId::Claude, ProviderState::Ok);
        claude.windows = vec![window("Session", 34.0, None)];
        claude.tokens = Some(TokenReport {
            today: TokenTotals {
                total: 1_234_567,
                cost_usd: Some(0.42),
                ..TokenTotals::default()
            },
            last7_days: TokenTotals {
                total: 812,
                ..TokenTotals::default()
            },
            by_model: vec![],
            updated_at: 0,
        });
        let text = render(&snapshot(vec![claude]), 0);
        assert!(text.ends_with("Claude Code · today 1.2M tokens · $0.42 · 7 days 812 tokens"));
    }

    #[test]
    fn non_ok_providers_show_their_state_and_message() {
        let mut codex = ProviderSnapshot::empty(ProviderId::Codex, ProviderState::NotInstalled);
        codex.message = Some("codex is not on PATH".into());
        let text = render(&snapshot(vec![codex]), 0);
        assert_eq!(text, "Codex · not installed — codex is not on PATH");

        let bare = ProviderSnapshot::empty(ProviderId::Codex, ProviderState::Unauthenticated);
        assert_eq!(render(&snapshot(vec![bare]), 0), "Codex · not signed in");
    }

    #[test]
    fn an_empty_snapshot_says_so() {
        assert_eq!(render(&snapshot(vec![]), 0), "No usage data yet");
    }

    #[test]
    fn counts_are_abbreviated() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(9_999), "9999");
        assert_eq!(format_count(10_000), "10.0k");
        assert_eq!(format_count(999_999), "1000.0k");
        assert_eq!(format_count(1_234_567), "1.2M");
    }

    #[test]
    fn past_reset_times_are_left_out() {
        assert_eq!(reset_suffix(Some(50), 100), "");
        assert_eq!(reset_suffix(None, 100), "");
        assert_eq!(reset_suffix(Some(3_700), 100), " · resets in 1h 0m");
    }

    #[test]
    fn defaults_are_available_without_a_config_file() {
        assert_eq!(default_config(), Config::default());
    }
}
