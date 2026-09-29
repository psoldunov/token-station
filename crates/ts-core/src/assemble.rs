//! Build the published [`Snapshot`] from provider snapshots and config.

use crate::config::Config;
use crate::meter::compute_meter;
use crate::snapshot::{Level, ProviderId, ProviderSnapshot, SCHEMA_VERSION, Snapshot, UsageWindow};

/// Order providers, classify every window against the thresholds and compute the meter.
#[must_use]
pub fn assemble(
    revision: u64,
    now: i64,
    providers: &[ProviderSnapshot],
    config: &Config,
) -> Snapshot {
    let alerts = &config.alerts;
    let mut ordered: Vec<ProviderSnapshot> = providers
        .iter()
        .map(|p| ProviderSnapshot {
            windows: p
                .windows
                .iter()
                .map(|w| UsageWindow {
                    level: Level::from_percent(
                        w.used_percent,
                        alerts.warning_percent,
                        alerts.critical_percent,
                    ),
                    ..w.clone()
                })
                .collect(),
            ..p.clone()
        })
        .collect();
    ordered.sort_by_key(|p| ProviderId::ALL.iter().position(|id| *id == p.id));
    let meter = compute_meter(&ordered, config.meter.window, alerts);
    Snapshot {
        schema_version: SCHEMA_VERSION,
        revision,
        generated_at: now,
        meter,
        providers: ordered,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{ProviderState, WindowKind};

    #[test]
    fn orders_providers_and_applies_levels() {
        let mut codex = ProviderSnapshot::empty(ProviderId::Codex, ProviderState::Ok);
        codex.windows = vec![UsageWindow {
            id: "codex:primary".into(),
            label: "Weekly".into(),
            kind: WindowKind::Weekly,
            used_percent: 85.0,
            resets_at: Some(10),
            window_minutes: Some(10080),
            level: Level::Normal,
            source: "app-server".into(),
            observed_at: 1,
        }];
        let claude = ProviderSnapshot::empty(ProviderId::Claude, ProviderState::Loading);
        let s = assemble(7, 100, &[codex, claude], &Config::default());
        assert_eq!(s.schema_version, SCHEMA_VERSION);
        assert_eq!(s.revision, 7);
        assert_eq!(s.generated_at, 100);
        assert_eq!(s.providers[0].id, ProviderId::Claude);
        assert_eq!(s.providers[1].windows[0].level, Level::Warning);
        assert_eq!(s.meter.bars[1].percent, Some(85.0));
        assert_eq!(s.meter.bars[0].percent, None);
        assert_eq!(s.meter.level, Level::Warning);
    }
}
