//! The words on the tray: tooltip text and the label items of the DBusMenu.
//!
//! Everything here is a pure function of the snapshot, so the wording is tested
//! against the recorded fixtures without a bus or a tray host.

use ts_core::{ProviderSnapshot, ProviderState, Snapshot};

use crate::status::{reset_suffix, state_text, totals_text};

/// Title of the item and of its tooltip.
pub const TITLE: &str = "Token Station";
/// Shown when the daemon could not be reached or activated.
pub const DAEMON_OFFLINE: &str = "Token Station daemon not running";
/// Indent for the window lines under a provider heading.
const INDENT: &str = "    ";

/// The label items of the menu, in the two groups the separators divide.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MenuText {
    /// One heading per provider, then one line per usage window.
    pub usage: Vec<String>,
    /// One token summary per provider that reported any.
    pub tokens: Vec<String>,
}

/// Does this provider currently carry usable numbers?
fn has_data(provider: &ProviderSnapshot) -> bool {
    matches!(provider.state, ProviderState::Ok | ProviderState::Stale)
        && !provider.windows.is_empty()
}

/// `Claude Code · Max 20x`, or `Claude Code · not signed in — …`.
fn heading(provider: &ProviderSnapshot) -> String {
    let detail = match (provider.state, provider.plan.as_deref()) {
        (ProviderState::Ok, Some(plan)) => plan.to_string(),
        (ProviderState::Ok, None) => String::new(),
        (state, _) => state_text(state, provider.message.as_deref()),
    };
    if detail.is_empty() {
        provider.name.clone()
    } else {
        format!("{} · {}", provider.name, detail)
    }
}

/// `Session · 34% · resets in 4h 40m`.
fn window_line(provider: &ProviderSnapshot, index: usize, now: i64) -> String {
    let window = &provider.windows[index];
    format!(
        "{INDENT}{} · {}%{}",
        window.label,
        window.used_percent.round() as i64,
        reset_suffix(window.resets_at, now),
    )
}

/// Every label item, ready to be turned into disabled `StandardItem`s.
pub fn menu_text(snapshot: Option<&Snapshot>, now: i64) -> MenuText {
    let Some(snapshot) = snapshot else {
        return MenuText {
            usage: vec![DAEMON_OFFLINE.to_string()],
            tokens: Vec::new(),
        };
    };
    let mut text = MenuText::default();
    for provider in &snapshot.providers {
        text.usage.push(heading(provider));
        if has_data(provider) {
            for index in 0..provider.windows.len() {
                text.usage.push(window_line(provider, index, now));
            }
        }
        if let Some(tokens) = &provider.tokens {
            text.tokens.push(format!(
                "{} · today {} · 7 days {}",
                provider.name,
                totals_text(&tokens.today),
                totals_text(&tokens.last7_days),
            ));
        }
    }
    if text.usage.is_empty() {
        text.usage.push("No usage data yet".into());
    }
    text
}

/// `Claude Code · Session 34% · resets in 2h 14m`, one line per provider.
pub fn tooltip_description(snapshot: Option<&Snapshot>, now: i64) -> String {
    let Some(snapshot) = snapshot else {
        return DAEMON_OFFLINE.to_string();
    };
    let lines: Vec<String> = snapshot
        .providers
        .iter()
        .map(
            |provider| match (has_data(provider), provider.most_constrained()) {
                (true, Some(window)) => format!(
                    "{} · {} {}%{}",
                    provider.name,
                    window.label,
                    window.used_percent.round() as i64,
                    reset_suffix(window.resets_at, now),
                ),
                _ => format!(
                    "{} · {}",
                    provider.name,
                    state_text(provider.state, provider.message.as_deref())
                ),
            },
        )
        .collect();
    if lines.is_empty() {
        return "No usage data yet".into();
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `generatedAt` of every fixture; the reset countdowns are relative to it.
    const NOW: i64 = 1_790_596_800;

    fn fixture(name: &str) -> Snapshot {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/fixtures/");
        let json = std::fs::read_to_string(format!("{path}{name}.json")).expect("fixture reads");
        serde_json::from_str(&json).expect("fixture parses")
    }

    #[test]
    fn a_healthy_snapshot_lists_every_window_and_both_summaries() {
        let text = menu_text(Some(&fixture("snapshot-ok")), NOW);
        assert_eq!(
            text.usage,
            vec![
                "Claude Code · Max 20x",
                "    Session · 34% · resets in 4h 40m",
                "    Weekly · 15% · resets in 4d 13h",
                "    Weekly · Fable · 0% · resets in 4d 13h",
                "Codex · Pro",
                "    Weekly · 12% · resets in 6d 1h",
            ]
        );
        assert_eq!(
            text.tokens,
            vec![
                "Claude Code · today 43.3M tokens · $31.42 · 7 days 317.2M tokens · $228.17",
                "Codex · today 680.4k tokens · $0.99 · 7 days 5.4M tokens · $7.42",
            ]
        );
    }

    #[test]
    fn the_tooltip_shows_the_most_constrained_window_per_provider() {
        assert_eq!(
            tooltip_description(Some(&fixture("snapshot-ok")), NOW),
            "Claude Code · Session 34% · resets in 4h 40m\nCodex · Weekly 12% · resets in 6d 1h"
        );
        assert_eq!(
            tooltip_description(Some(&fixture("snapshot-near-limit")), NOW),
            "Claude Code · Session 96% · resets in 4h 40m\nCodex · Weekly 88% · resets in 6d 1h"
        );
    }

    #[test]
    fn broken_providers_say_why_instead_of_showing_numbers() {
        let text = menu_text(Some(&fixture("snapshot-not-installed")), NOW);
        assert!(
            text.usage
                .contains(&"Codex · not installed — Codex CLI not found.".to_string())
        );
        assert_eq!(
            tooltip_description(Some(&fixture("snapshot-stale-auth")), NOW),
            "Claude Code · Session 34% · resets in 4h 40m\n\
             Codex · not signed in — Codex is not signed in. Run `codex login`."
        );
    }

    #[test]
    fn a_loading_snapshot_has_headings_but_no_windows() {
        let text = menu_text(Some(&fixture("snapshot-loading")), NOW);
        assert_eq!(text.usage, vec!["Claude Code · loading", "Codex · loading"]);
        assert!(text.tokens.is_empty());
        assert_eq!(
            tooltip_description(Some(&fixture("snapshot-loading")), NOW),
            "Claude Code · loading\nCodex · loading"
        );
    }

    #[test]
    fn without_a_daemon_both_surfaces_say_so() {
        assert_eq!(menu_text(None, NOW).usage, vec![DAEMON_OFFLINE]);
        assert!(menu_text(None, NOW).tokens.is_empty());
        assert_eq!(tooltip_description(None, NOW), DAEMON_OFFLINE);
    }

    #[test]
    fn an_empty_provider_list_still_produces_a_line() {
        let mut snapshot = fixture("snapshot-ok");
        snapshot.providers.clear();
        assert_eq!(
            menu_text(Some(&snapshot), NOW).usage,
            vec!["No usage data yet"]
        );
        assert_eq!(
            tooltip_description(Some(&snapshot), NOW),
            "No usage data yet"
        );
    }
}
