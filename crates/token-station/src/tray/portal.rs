//! The desktop colour scheme, read from the freedesktop appearance portal.
//!
//! `org.freedesktop.portal.Settings.Read("org.freedesktop.appearance", "color-scheme")`
//! answers `0` (no preference), `1` (prefer dark) or `2` (prefer light). Portals of
//! different vintages wrap the answer in one or two variants, so the parsing walks
//! through them. A missing portal is not an error: the tray keeps the documented
//! default of dark bars on a light panel.

use zbus::zvariant::Value;

use crate::tray::palette::ColorScheme;

/// Namespace the appearance settings live in.
pub const APPEARANCE: &str = "org.freedesktop.appearance";
/// Key holding the light/dark preference.
pub const COLOR_SCHEME: &str = "color-scheme";

/// Client side of the settings portal.
#[zbus::proxy(
    interface = "org.freedesktop.portal.Settings",
    default_service = "org.freedesktop.portal.Desktop",
    default_path = "/org/freedesktop/portal/desktop",
    gen_blocking = false
)]
pub trait Settings {
    fn read(&self, namespace: &str, key: &str) -> zbus::Result<zbus::zvariant::OwnedValue>;

    #[zbus(signal)]
    fn setting_changed(
        &self,
        namespace: String,
        key: String,
        value: zbus::zvariant::OwnedValue,
    ) -> zbus::Result<()>;
}

/// Unwrap however many variants the portal nested the number in.
pub fn scheme_from_value(value: &Value<'_>) -> Option<ColorScheme> {
    match value {
        Value::U32(number) => Some(ColorScheme::from_portal(*number)),
        Value::U8(number) => Some(ColorScheme::from_portal(u32::from(*number))),
        Value::I32(number) => u32::try_from(*number).ok().map(ColorScheme::from_portal),
        Value::U64(number) => u32::try_from(*number).ok().map(ColorScheme::from_portal),
        Value::Value(inner) => scheme_from_value(inner),
        _ => None,
    }
}

/// Ask the portal once; `None` when it is absent or answers something unreadable.
pub async fn read_scheme(connection: &zbus::Connection) -> Option<ColorScheme> {
    let proxy = SettingsProxy::new(connection).await.ok()?;
    let value = proxy.read(APPEARANCE, COLOR_SCHEME).await.ok()?;
    scheme_from_value(&value)
}

/// Is this `SettingChanged` about the colour scheme?
#[must_use]
pub fn is_color_scheme(namespace: &str, key: &str) -> bool {
    namespace == APPEARANCE && key == COLOR_SCHEME
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_number_is_read_directly() {
        assert_eq!(
            scheme_from_value(&Value::U32(1)),
            Some(ColorScheme::Dark),
            "1 means prefer-dark"
        );
        assert_eq!(scheme_from_value(&Value::U32(2)), Some(ColorScheme::Light));
        assert_eq!(scheme_from_value(&Value::U32(0)), Some(ColorScheme::Light));
    }

    #[test]
    fn nested_variants_are_unwrapped() {
        let once = Value::Value(Box::new(Value::U32(1)));
        assert_eq!(scheme_from_value(&once), Some(ColorScheme::Dark));
        let twice = Value::Value(Box::new(Value::Value(Box::new(Value::U32(2)))));
        assert_eq!(scheme_from_value(&twice), Some(ColorScheme::Light));
    }

    #[test]
    fn other_integer_widths_still_parse() {
        assert_eq!(scheme_from_value(&Value::U8(1)), Some(ColorScheme::Dark));
        assert_eq!(scheme_from_value(&Value::I32(1)), Some(ColorScheme::Dark));
        assert_eq!(scheme_from_value(&Value::U64(1)), Some(ColorScheme::Dark));
        // A negative preference is not a portal value we know.
        assert_eq!(scheme_from_value(&Value::I32(-1)), None);
    }

    #[test]
    fn nonsense_is_rejected_rather_than_guessed() {
        assert_eq!(scheme_from_value(&Value::Str("dark".into())), None);
        assert_eq!(scheme_from_value(&Value::Bool(true)), None);
    }

    #[test]
    fn only_the_colour_scheme_key_matters() {
        assert!(is_color_scheme(APPEARANCE, COLOR_SCHEME));
        assert!(!is_color_scheme(APPEARANCE, "accent-color"));
        assert!(!is_color_scheme(
            "org.gnome.desktop.interface",
            COLOR_SCHEME
        ));
    }
}
