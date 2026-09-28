//! The tray mini-meter: one bar per provider.

use crate::config::{AlertsConfig, MeterWindow};
use crate::snapshot::{
    Level, Meter, MeterBar, ProviderSnapshot, ProviderState, UsageWindow, WindowKind,
};

/// Pick the window a provider's bar represents.
pub fn meter_window(provider: &ProviderSnapshot, choice: MeterWindow) -> Option<&UsageWindow> {
    let by_kind = |kind: WindowKind| {
        provider
            .windows
            .iter()
            .filter(|w| w.kind == kind)
            .max_by(|a, b| a.used_percent.total_cmp(&b.used_percent))
    };
    match choice {
        MeterWindow::MostConstrained => provider.most_constrained(),
        MeterWindow::Session => {
            by_kind(WindowKind::Session).or_else(|| provider.most_constrained())
        }
        MeterWindow::Weekly => by_kind(WindowKind::Weekly).or_else(|| provider.most_constrained()),
    }
}

/// Bar for one provider. Providers without usable data get an empty bar.
pub fn meter_bar(
    provider: &ProviderSnapshot,
    choice: MeterWindow,
    alerts: &AlertsConfig,
) -> MeterBar {
    let usable = matches!(provider.state, ProviderState::Ok | ProviderState::Stale);
    let window = usable.then(|| meter_window(provider, choice)).flatten();
    MeterBar {
        provider: provider.id,
        percent: window.map(|w| w.used_percent),
        level: window.map_or(Level::Normal, |w| {
            Level::from_percent(
                w.used_percent,
                alerts.warning_percent,
                alerts.critical_percent,
            )
        }),
        window_id: window.map(|w| w.id.clone()),
    }
}

/// Meter over all enabled providers (disabled ones are omitted).
pub fn compute_meter(
    providers: &[ProviderSnapshot],
    choice: MeterWindow,
    alerts: &AlertsConfig,
) -> Meter {
    let bars: Vec<MeterBar> = providers
        .iter()
        .filter(|p| p.state != ProviderState::Disabled)
        .map(|p| meter_bar(p, choice, alerts))
        .collect();
    let level = bars.iter().map(|b| b.level).max().unwrap_or_default();
    Meter { bars, level }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::ProviderId;

    fn w(id: &str, kind: WindowKind, pct: f64) -> UsageWindow {
        UsageWindow {
            id: id.into(),
            label: id.into(),
            kind,
            used_percent: pct,
            resets_at: None,
            window_minutes: None,
            level: Level::Normal,
            source: "test".into(),
            observed_at: 0,
        }
    }

    fn provider(
        id: ProviderId,
        state: ProviderState,
        windows: Vec<UsageWindow>,
    ) -> ProviderSnapshot {
        ProviderSnapshot {
            windows,
            ..ProviderSnapshot::empty(id, state)
        }
    }

    #[test]
    fn picks_requested_window_with_fallback() {
        let p = provider(
            ProviderId::Claude,
            ProviderState::Ok,
            vec![
                w("session", WindowKind::Session, 20.0),
                w("weekly", WindowKind::Weekly, 60.0),
            ],
        );
        assert_eq!(
            meter_window(&p, MeterWindow::MostConstrained).unwrap().id,
            "weekly"
        );
        assert_eq!(
            meter_window(&p, MeterWindow::Session).unwrap().id,
            "session"
        );
        let only_weekly = provider(
            ProviderId::Codex,
            ProviderState::Ok,
            vec![w("weekly", WindowKind::Weekly, 5.0)],
        );
        assert_eq!(
            meter_window(&only_weekly, MeterWindow::Session).unwrap().id,
            "weekly"
        );
    }

    #[test]
    fn bars_follow_state_and_levels() {
        let alerts = AlertsConfig::default();
        let providers = vec![
            provider(
                ProviderId::Claude,
                ProviderState::Stale,
                vec![w("s", WindowKind::Session, 96.0)],
            ),
            provider(
                ProviderId::Codex,
                ProviderState::Unauthenticated,
                vec![w("s", WindowKind::Session, 50.0)],
            ),
        ];
        let m = compute_meter(&providers, MeterWindow::MostConstrained, &alerts);
        assert_eq!(m.bars.len(), 2);
        assert_eq!(m.bars[0].percent, Some(96.0));
        assert_eq!(m.bars[0].level, Level::Critical);
        assert_eq!(m.bars[1].percent, None);
        assert_eq!(m.level, Level::Critical);
    }

    #[test]
    fn disabled_providers_are_omitted() {
        let providers = vec![
            provider(ProviderId::Claude, ProviderState::Disabled, vec![]),
            provider(
                ProviderId::Codex,
                ProviderState::Ok,
                vec![w("s", WindowKind::Weekly, 81.0)],
            ),
        ];
        let m = compute_meter(
            &providers,
            MeterWindow::MostConstrained,
            &AlertsConfig::default(),
        );
        assert_eq!(m.bars.len(), 1);
        assert_eq!(m.bars[0].provider, ProviderId::Codex);
        assert_eq!(m.level, Level::Warning);
    }

    #[test]
    fn empty_meter_is_normal() {
        let m = compute_meter(&[], MeterWindow::Weekly, &AlertsConfig::default());
        assert!(m.bars.is_empty());
        assert_eq!(m.level, Level::Normal);
    }
}
