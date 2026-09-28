//! The words on the tray: tooltip text and the label items of the DBusMenu.
//!
//! Everything here is a pure function of the snapshot, so the wording is tested
//! against the recorded fixtures without a bus or a tray host.

use ts_core::{ProviderSnapshot, ProviderState, Snapshot, UsageWindow};

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
fn window_line(window: &UsageWindow, now: i64) -> String {
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
            for window in &provider.windows {
                text.usage.push(window_line(window, now));
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

    /// One fixture rendered through `menu_text`/`tooltip_description`, with only the
    /// fields a given case cares about checked.
    #[derive(Default)]
    struct MenuTextCase {
        name: &'static str,
        fixture: &'static str,
        usage_exact: Option<Vec<&'static str>>,
        usage_contains: Option<&'static str>,
        tokens_exact: Option<Vec<&'static str>>,
        tokens_empty: bool,
        tooltip: Option<&'static str>,
    }

    #[test]
    fn menu_text_and_tooltip_render_each_fixture_as_expected() {
        let cases = [
            MenuTextCase {
                name: "a healthy snapshot lists every window and both summaries",
                fixture: "snapshot-ok",
                usage_exact: Some(vec![
                    "Claude Code · Max 20x",
                    "    Session · 34% · resets in 4h 40m",
                    "    Weekly · 15% · resets in 4d 13h",
                    "    Weekly · Fable · 0% · resets in 4d 13h",
                    "Codex · Pro",
                    "    Weekly · 12% · resets in 6d 1h",
                ]),
                tokens_exact: Some(vec![
                    "Claude Code · today 43.3M tokens · $31.42 · 7 days 317.2M tokens · $228.17",
                    "Codex · today 680.4k tokens · $0.99 · 7 days 5.4M tokens · $7.42",
                ]),
                ..Default::default()
            },
            MenuTextCase {
                name: "a loading snapshot has headings but no windows",
                fixture: "snapshot-loading",
                usage_exact: Some(vec!["Claude Code · loading", "Codex · loading"]),
                tokens_empty: true,
                tooltip: Some("Claude Code · loading\nCodex · loading"),
                ..Default::default()
            },
            MenuTextCase {
                name: "a not-installed provider says why instead of showing numbers",
                fixture: "snapshot-not-installed",
                usage_contains: Some("Codex · not installed — Codex CLI not found."),
                ..Default::default()
            },
            MenuTextCase {
                name: "a stale-auth tooltip explains why",
                fixture: "snapshot-stale-auth",
                tooltip: Some(
                    "Claude Code · Session 34% · resets in 4h 40m\n\
                     Codex · not signed in — Codex is not signed in. Run `codex login`.",
                ),
                ..Default::default()
            },
        ];

        for case in cases {
            let snapshot = fixture(case.fixture);
            let text = menu_text(Some(&snapshot), NOW);
            if let Some(expected) = &case.usage_exact {
                let expected: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
                assert_eq!(text.usage, expected, "{}: usage", case.name);
            }
            if let Some(needle) = case.usage_contains {
                assert!(
                    text.usage.contains(&needle.to_string()),
                    "{}: usage should contain {needle:?}",
                    case.name
                );
            }
            if let Some(expected) = &case.tokens_exact {
                let expected: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
                assert_eq!(text.tokens, expected, "{}: tokens", case.name);
            }
            if case.tokens_empty {
                assert!(
                    text.tokens.is_empty(),
                    "{}: tokens should be empty",
                    case.name
                );
            }
            if let Some(expected) = case.tooltip {
                assert_eq!(
                    tooltip_description(Some(&snapshot), NOW),
                    expected,
                    "{}: tooltip",
                    case.name
                );
            }
        }
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
