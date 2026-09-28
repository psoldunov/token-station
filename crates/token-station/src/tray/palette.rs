//! Colours for the tray meter, and the desktop colour scheme they follow.
//!
//! The tray is the fallback front end for desktops that are neither KDE nor GNOME,
//! so neither native palette is the "correct" one. The Breeze accents win here
//! because they stay legible at 16 px on both light and dark panels: Adwaita's
//! amber (`#e5a50a`) washes out against a light panel, while Breeze's `#f67400`
//! keeps its contrast. The reds are close enough that the choice is cosmetic.

use ts_core::Level;

/// Straight (non-premultiplied) 8-bit colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
        Rgba { r, g, b, a: 255 }
    }

    /// The same colour at `alpha` (0–255).
    pub const fn with_alpha(self, alpha: u8) -> Rgba {
        Rgba { a: alpha, ..self }
    }
}

/// Breeze "text" on a light panel.
pub const DARK_FOREGROUND: Rgba = Rgba::rgb(0x23, 0x26, 0x29);
/// Breeze "text" on a dark panel.
pub const LIGHT_FOREGROUND: Rgba = Rgba::rgb(0xef, 0xf0, 0xf1);
/// Breeze "neutral" (amber).
pub const WARNING: Rgba = Rgba::rgb(0xf6, 0x74, 0x00);
/// Breeze "negative" (red).
pub const CRITICAL: Rgba = Rgba::rgb(0xda, 0x44, 0x53);

/// Opacity of the empty part of a bar, relative to the foreground.
pub const TRACK_ALPHA: u8 = 0x59;

/// What the desktop told us through `org.freedesktop.appearance`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ColorScheme {
    /// "Prefer dark": the panel is dark, so the bars are drawn light.
    Dark,
    /// "Prefer light" or "no preference": dark bars on a light panel.
    #[default]
    Light,
}

impl ColorScheme {
    /// The portal reports `1` for "prefer dark"; every other value means light.
    pub fn from_portal(value: u32) -> ColorScheme {
        match value {
            1 => ColorScheme::Dark,
            _ => ColorScheme::Light,
        }
    }

    /// Colour of a filled bar at `Level::Normal`, and of every bar outline.
    pub fn foreground(self) -> Rgba {
        match self {
            ColorScheme::Dark => LIGHT_FOREGROUND,
            ColorScheme::Light => DARK_FOREGROUND,
        }
    }
}

/// The colours one bar is drawn with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarColors {
    /// Outline and empty track.
    pub track: Rgba,
    /// The filled part.
    pub fill: Rgba,
}

/// Outline plus fill for one bar at `level`.
pub fn bar_colors(scheme: ColorScheme, level: Level) -> BarColors {
    let foreground = scheme.foreground();
    let fill = match level {
        Level::Normal => foreground,
        Level::Warning => WARNING,
        Level::Critical => CRITICAL,
    };
    BarColors {
        track: foreground.with_alpha(TRACK_ALPHA),
        fill,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_means_dark() {
        assert_eq!(ColorScheme::from_portal(1), ColorScheme::Dark);
        for value in [0, 2, 3, 99] {
            assert_eq!(
                ColorScheme::from_portal(value),
                ColorScheme::Light,
                "{value}"
            );
        }
        // The documented default when the portal is missing.
        assert_eq!(ColorScheme::default(), ColorScheme::Light);
    }

    #[test]
    fn a_dark_panel_gets_light_bars() {
        assert_eq!(ColorScheme::Dark.foreground(), LIGHT_FOREGROUND);
        assert_eq!(ColorScheme::Light.foreground(), DARK_FOREGROUND);
    }

    #[test]
    fn levels_pick_the_accent_and_keep_the_track() {
        for scheme in [ColorScheme::Dark, ColorScheme::Light] {
            let foreground = scheme.foreground();
            assert_eq!(
                bar_colors(scheme, Level::Normal),
                BarColors {
                    track: foreground.with_alpha(TRACK_ALPHA),
                    fill: foreground,
                }
            );
            assert_eq!(bar_colors(scheme, Level::Warning).fill, WARNING);
            assert_eq!(bar_colors(scheme, Level::Critical).fill, CRITICAL);
            // The outline never changes with the level.
            assert_eq!(
                bar_colors(scheme, Level::Critical).track,
                foreground.with_alpha(TRACK_ALPHA)
            );
        }
    }
}
