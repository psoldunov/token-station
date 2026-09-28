//! Desktop notifications for threshold crossings.
//!
//! The text is built by pure functions so it can be asserted without a notification
//! daemon; [`Notifier`] is the seam the tests replace.
//!
//! The call goes straight to `org.freedesktop.Notifications` over the session bus.
//! Convenience wrappers around that interface tend to build a runtime of their own,
//! which panics ("cannot start a runtime from within a runtime") the moment an
//! alert fires from a tokio worker thread, so the daemon owns the proxy instead.

use std::collections::HashMap;

use async_trait::async_trait;
use ts_core::Level;
use ts_core::alerts::{Alert, AlertKind};
use zbus::zvariant::Value;

/// Desktop entry / icon name shared with the `.desktop` file.
pub const APP_ID: &str = "dev.soldunov.TokenStation";
pub const APP_NAME: &str = "Token Station";

/// How long a notification stays up; `-1` leaves it to the server's default.
const EXPIRE_DEFAULT: i32 = -1;

/// Where the notification server listens.
pub const NOTIFICATIONS_SERVICE: &str = "org.freedesktop.Notifications";
pub const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";

/// Something that can show a desktop notification.
#[async_trait]
pub trait Notifier: Send + Sync {
    async fn notify(&self, summary: &str, body: &str, level: Level);
}

/// Sends through the session bus, on the connection the daemon already owns.
pub struct DesktopNotifier {
    connection: zbus::Connection,
}

impl DesktopNotifier {
    pub fn new(connection: zbus::Connection) -> DesktopNotifier {
        DesktopNotifier { connection }
    }

    async fn send(&self, summary: &str, body: &str, level: Level) -> zbus::Result<u32> {
        let proxy = zbus::Proxy::new(
            &self.connection,
            NOTIFICATIONS_SERVICE,
            NOTIFICATIONS_PATH,
            NOTIFICATIONS_SERVICE,
        )
        .await?;
        let hints: HashMap<&str, Value<'_>> = HashMap::from([
            ("urgency", Value::U8(urgency(level))),
            ("desktop-entry", Value::from(APP_ID)),
        ]);
        proxy
            .call(
                "Notify",
                &(
                    APP_NAME,
                    0u32,
                    APP_ID,
                    summary,
                    body,
                    Vec::<&str>::new(),
                    hints,
                    EXPIRE_DEFAULT,
                ),
            )
            .await
    }
}

#[async_trait]
impl Notifier for DesktopNotifier {
    async fn notify(&self, summary: &str, body: &str, level: Level) {
        if let Err(error) = self.send(summary, body, level).await {
            tracing::warn!(%error, "cannot show desktop notification");
        }
    }
}

/// The `urgency` hint the notification spec defines.
fn urgency(level: Level) -> u8 {
    match level {
        Level::Critical => 2,
        Level::Warning | Level::Normal => 1,
    }
}

/// Drops every notification; used when alerts are off and in tests.
pub struct SilentNotifier;

#[async_trait]
impl Notifier for SilentNotifier {
    async fn notify(&self, _summary: &str, _body: &str, _level: Level) {}
}

/// Summary, body and urgency for one alert, as seen by the user.
pub fn alert_text(alert: &Alert, now: i64) -> (String, String, Level) {
    let who = alert.provider.display_name();
    let percent = alert.percent.round() as i64;
    match alert.kind {
        AlertKind::Reset => (
            format!("{who}: {} reset", alert.label),
            format!("Usage is back to {percent} %"),
            Level::Normal,
        ),
        AlertKind::Warning | AlertKind::Critical => {
            let level = match alert.kind {
                AlertKind::Critical => Level::Critical,
                _ => Level::Warning,
            };
            (
                format!("{who}: {} at {percent} %", alert.label),
                reset_body(alert.resets_at, now),
                level,
            )
        }
    }
}

fn reset_body(resets_at: Option<i64>, now: i64) -> String {
    match resets_at.map(|t| t - now) {
        Some(remaining) if remaining > 0 => format!("Resets in {}", long_duration(remaining)),
        Some(_) => "Resets now".into(),
        None => "No reset time reported".into(),
    }
}

/// Spelled-out duration for notification bodies: `2 h 14 min`.
pub fn long_duration(seconds: i64) -> String {
    format_duration(seconds, " ", "d", "h", "min")
}

/// Tight duration for one-line CLI output: `2h 14m`.
pub fn compact_duration(seconds: i64) -> String {
    format_duration(seconds, "", "d", "h", "m")
}

fn format_duration(seconds: i64, gap: &str, d: &str, h: &str, m: &str) -> String {
    let seconds = seconds.max(0);
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let unit = |value: i64, suffix: &str| format!("{value}{gap}{suffix}");
    if days > 0 {
        format!("{} {}", unit(days, d), unit(hours, h))
    } else if hours > 0 {
        format!("{} {}", unit(hours, h), unit(minutes, m))
    } else {
        unit(minutes.max(if seconds > 0 { 1 } else { 0 }), m)
    }
}

/// Show every alert.
pub async fn deliver(notifier: &dyn Notifier, alerts: &[Alert], now: i64) {
    for alert in alerts {
        let (summary, body, level) = alert_text(alert, now);
        tracing::info!(%summary, %body, "alert");
        notifier.notify(&summary, &body, level).await;
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::Mutex;

    use super::*;

    /// Collects notifications instead of showing them.
    #[derive(Default)]
    pub struct RecordingNotifier {
        pub sent: Mutex<Vec<(String, String, Level)>>,
    }

    #[async_trait]
    impl Notifier for RecordingNotifier {
        async fn notify(&self, summary: &str, body: &str, level: Level) {
            let mut sent = self.sent.lock().unwrap_or_else(|e| e.into_inner());
            sent.push((summary.into(), body.into(), level));
        }
    }

    impl RecordingNotifier {
        pub fn summaries(&self) -> Vec<String> {
            let sent = self.sent.lock().unwrap_or_else(|e| e.into_inner());
            sent.iter().map(|(s, _, _)| s.clone()).collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::RecordingNotifier;
    use super::*;
    use ts_core::ProviderId;

    fn alert(kind: AlertKind, percent: f64, resets_at: Option<i64>) -> Alert {
        Alert {
            provider: ProviderId::Claude,
            window_id: "session".into(),
            label: "Session".into(),
            kind,
            percent,
            resets_at,
        }
    }

    #[test]
    fn warning_text_matches_the_spec() {
        let now = 1_790_596_800;
        let (summary, body, level) =
            alert_text(&alert(AlertKind::Warning, 82.4, Some(now + 8_040)), now);
        assert_eq!(summary, "Claude Code: Session at 82 %");
        assert_eq!(body, "Resets in 2 h 14 min");
        assert_eq!(level, Level::Warning);
    }

    #[test]
    fn critical_is_urgent_and_reset_is_not() {
        let (_, _, level) = alert_text(&alert(AlertKind::Critical, 96.0, Some(10)), 0);
        assert_eq!(level, Level::Critical);
        let (summary, body, level) = alert_text(&alert(AlertKind::Reset, 1.4, Some(10_000)), 0);
        assert_eq!(summary, "Claude Code: Session reset");
        assert_eq!(body, "Usage is back to 1 %");
        assert_eq!(level, Level::Normal);
    }

    #[test]
    fn the_urgency_hint_follows_the_level() {
        assert_eq!(urgency(Level::Critical), 2);
        assert_eq!(urgency(Level::Warning), 1);
        assert_eq!(urgency(Level::Normal), 1);
    }

    #[test]
    fn missing_or_past_reset_times_read_sensibly() {
        assert_eq!(
            alert_text(&alert(AlertKind::Warning, 80.0, None), 0).1,
            "No reset time reported"
        );
        assert_eq!(
            alert_text(&alert(AlertKind::Warning, 80.0, Some(50)), 100).1,
            "Resets now"
        );
    }

    #[test]
    fn durations_cover_days_hours_and_minutes() {
        assert_eq!(long_duration(8_040), "2 h 14 min");
        assert_eq!(long_duration(2_700), "45 min");
        assert_eq!(long_duration(273_600), "3 d 4 h");
        assert_eq!(long_duration(30), "1 min");
        assert_eq!(long_duration(0), "0 min");
        assert_eq!(long_duration(-5), "0 min");
        assert_eq!(compact_duration(8_040), "2h 14m");
        assert_eq!(compact_duration(273_600), "3d 4h");
    }

    #[tokio::test]
    async fn deliver_sends_one_notification_per_alert() {
        let notifier = RecordingNotifier::default();
        deliver(
            &notifier,
            &[
                alert(AlertKind::Warning, 81.0, Some(3_600)),
                alert(AlertKind::Critical, 99.0, Some(3_600)),
            ],
            0,
        )
        .await;
        assert_eq!(
            notifier.summaries(),
            vec![
                "Claude Code: Session at 81 %".to_string(),
                "Claude Code: Session at 99 %".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn silent_notifier_does_nothing() {
        SilentNotifier.notify("a", "b", Level::Critical).await;
    }
}
