//! Desktop detection, and the entries and icons every desktop gets.
//!
//! The `.desktop` files and the icons are compiled into the binary rather than
//! read from the payload: `setup` has to rewrite `Exec=` to the real path of the
//! running AppImage anyway, and embedding them means `--payload-dir` only ever
//! has to hold the two front-end payloads.

use std::path::{Path, PathBuf};

use crate::integration::Dirs;
use crate::integration::step::Step;

/// Application entry, shown by launchers.
pub const ENTRY: &str = "dev.soldunov.TokenStation.desktop";
/// Autostart entry for the tray, used on desktops with no native front end.
pub const TRAY_ENTRY: &str = "dev.soldunov.TokenStation.Tray.desktop";

const ENTRY_TEMPLATE: &str =
    include_str!("../../../../data/applications/dev.soldunov.TokenStation.desktop");
const TRAY_TEMPLATE: &str =
    include_str!("../../../../data/applications/dev.soldunov.TokenStation.Tray.desktop");

/// Icons, as (path under `icons/`, contents).
const ICONS: [(&str, &str); 2] = [
    (
        "hicolor/scalable/apps/dev.soldunov.TokenStation.svg",
        include_str!("../../../../data/icons/hicolor/scalable/apps/dev.soldunov.TokenStation.svg"),
    ),
    (
        "hicolor/symbolic/apps/dev.soldunov.TokenStation-symbolic.svg",
        include_str!(
            "../../../../data/icons/hicolor/symbolic/apps/dev.soldunov.TokenStation-symbolic.svg"
        ),
    ),
];

/// Which front end `setup` should install.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Desktop {
    Kde,
    Gnome,
    /// Anything else: the SNI tray is the front end.
    #[default]
    Other,
}

impl Desktop {
    pub fn as_str(self) -> &'static str {
        match self {
            Desktop::Kde => "kde",
            Desktop::Gnome => "gnome",
            Desktop::Other => "other",
        }
    }
}

/// Read `$XDG_CURRENT_DESKTOP`, which is a colon-separated list.
pub fn detect(current: Option<&str>) -> Desktop {
    let names = current.unwrap_or_default().to_ascii_uppercase();
    let mut parts = names.split(':').map(str::trim);
    parts
        .find_map(|name| match name {
            "KDE" | "PLASMA" => Some(Desktop::Kde),
            "GNOME" | "GNOME-CLASSIC" | "GNOME-FLASHBACK" => Some(Desktop::Gnome),
            _ => None,
        })
        .unwrap_or_default()
}

/// Replace the `Exec=` line with the real binary and `args`.
pub fn rewrite_exec(contents: &str, exec: &Path, args: &str) -> String {
    let replacement = format!("Exec=\"{}\" {args}", exec.display());
    let mut out: Vec<String> = contents
        .lines()
        .map(|line| {
            if line.starts_with("Exec=") {
                replacement.clone()
            } else {
                line.to_string()
            }
        })
        .collect();
    if !out.iter().any(|line| line.starts_with("Exec=")) {
        out.push(replacement);
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// The application entry and the icons, for every desktop.
pub fn shared_steps(dirs: &Dirs, exec: &Path) -> Vec<Step> {
    let mut steps = vec![Step::Write {
        target: dirs.data.join("applications").join(ENTRY),
        contents: rewrite_exec(ENTRY_TEMPLATE, exec, "tray"),
    }];
    steps.extend(ICONS.iter().map(|(relative, contents)| Step::Write {
        target: dirs.data.join("icons").join(relative),
        contents: (*contents).to_string(),
    }));
    steps
}

/// XDG autostart entry that starts the tray at login.
pub fn autostart_step(dirs: &Dirs, exec: &Path) -> Step {
    Step::Write {
        target: autostart_path(dirs),
        contents: rewrite_exec(TRAY_TEMPLATE, exec, "tray"),
    }
}

/// Where the autostart entry lives.
pub fn autostart_path(dirs: &Dirs) -> PathBuf {
    dirs.config.join("autostart").join(TRAY_ENTRY)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs() -> Dirs {
        Dirs {
            home: "/home/u".into(),
            data: "/da".into(),
            config: "/cfg".into(),
            state: "/st".into(),
        }
    }

    #[test]
    fn the_desktop_is_read_from_the_session_variable() {
        assert_eq!(detect(Some("KDE")), Desktop::Kde);
        assert_eq!(detect(Some("plasma")), Desktop::Kde);
        assert_eq!(detect(Some("ubuntu:GNOME")), Desktop::Gnome);
        assert_eq!(detect(Some("GNOME-Classic:GNOME")), Desktop::Gnome);
        assert_eq!(detect(Some("sway")), Desktop::Other);
        assert_eq!(detect(Some("")), Desktop::Other);
        assert_eq!(detect(None), Desktop::Other);
        assert_eq!(Desktop::default(), Desktop::Other);
    }

    #[test]
    fn desktop_names_round_trip_through_the_manifest_spelling() {
        assert_eq!(Desktop::Kde.as_str(), "kde");
        assert_eq!(Desktop::Gnome.as_str(), "gnome");
        assert_eq!(Desktop::Other.as_str(), "other");
    }

    #[test]
    fn the_exec_line_is_quoted_and_everything_else_survives() {
        let rewritten = rewrite_exec(
            ENTRY_TEMPLATE,
            Path::new("/apps/Token Station.AppImage"),
            "tray",
        );
        assert!(
            rewritten.contains("Exec=\"/apps/Token Station.AppImage\" tray"),
            "{rewritten}"
        );
        assert!(rewritten.contains("Name=Token Station"));
        assert!(rewritten.contains("Icon=dev.soldunov.TokenStation"));
        assert_eq!(rewritten.matches("Exec=").count(), 1);
        assert!(rewritten.ends_with('\n'));
    }

    #[test]
    fn an_entry_without_an_exec_line_gets_one() {
        let rewritten = rewrite_exec("[Desktop Entry]\nType=Application", Path::new("/x"), "tray");
        assert_eq!(
            rewritten,
            "[Desktop Entry]\nType=Application\nExec=\"/x\" tray\n"
        );
    }

    #[test]
    fn shared_steps_write_the_entry_and_both_icons() {
        let steps = shared_steps(&dirs(), Path::new("/x"));
        let targets: Vec<String> = steps
            .iter()
            .map(|step| match step {
                Step::Write { target, .. } => target.display().to_string(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            targets,
            vec![
                "/da/applications/dev.soldunov.TokenStation.desktop",
                "/da/icons/hicolor/scalable/apps/dev.soldunov.TokenStation.svg",
                "/da/icons/hicolor/symbolic/apps/dev.soldunov.TokenStation-symbolic.svg",
            ]
        );
    }

    #[test]
    fn the_autostart_entry_starts_the_tray() {
        let step = autostart_step(&dirs(), Path::new("/x.AppImage"));
        let Step::Write { target, contents } = step else {
            panic!("expected a write");
        };
        assert_eq!(
            target,
            Path::new("/cfg/autostart/dev.soldunov.TokenStation.Tray.desktop")
        );
        assert!(contents.contains("Exec=\"/x.AppImage\" tray"), "{contents}");
        assert!(contents.contains("NotShowIn=KDE;GNOME;"));
    }
}
