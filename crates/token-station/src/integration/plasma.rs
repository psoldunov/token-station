//! The Plasma applet: a plasmoid directory copied into the user's data home.

use std::path::{Path, PathBuf};

use crate::integration::Dirs;
use crate::integration::step::Step;

/// Directory name of the applet, inside the payload and in the data home.
pub const APPLET_ID: &str = "dev.soldunov.tokenstation";
/// Name of the payload directory holding the applet.
pub const PAYLOAD: &str = "plasma";

/// Where the applet is installed.
pub fn target(dirs: &Dirs) -> PathBuf {
    dirs.data.join("plasma/plasmoids").join(APPLET_ID)
}

/// Copy the applet out of `payload` (the `integrations` directory).
pub fn steps(payload: &Path, dirs: &Dirs) -> anyhow::Result<Vec<Step>> {
    let source = payload.join(PAYLOAD);
    if !source.is_dir() {
        anyhow::bail!(
            "no Plasma applet at {}; pass --payload-dir or use the AppImage",
            source.display()
        );
    }
    let target = target(dirs);
    Ok(vec![
        Step::CopyTree {
            source,
            target: target.clone(),
        },
        Step::Note(format!(
            "Plasma applet installed at {}; add it from the panel's \"Add Widgets…\" menu.",
            target.display()
        )),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(root: &Path) -> Dirs {
        Dirs {
            home: root.into(),
            data: root.join("data"),
            config: root.join("config"),
            state: root.join("state"),
        }
    }

    #[test]
    fn the_applet_lands_where_plasma_looks_for_plasmoids() {
        let dirs = Dirs {
            home: "/home/u".into(),
            data: "/da".into(),
            config: "/cfg".into(),
            state: "/st".into(),
        };
        assert_eq!(
            target(&dirs),
            Path::new("/da/plasma/plasmoids/dev.soldunov.tokenstation")
        );
    }

    #[test]
    fn a_present_payload_produces_a_copy_and_a_hint() {
        let root = tempfile::tempdir().unwrap();
        let payload = root.path().join("integrations");
        std::fs::create_dir_all(payload.join(PAYLOAD)).unwrap();
        let dirs = dirs(root.path());

        let steps = steps(&payload, &dirs).unwrap();
        assert_eq!(
            steps[0],
            Step::CopyTree {
                source: payload.join(PAYLOAD),
                target: target(&dirs),
            }
        );
        assert!(matches!(&steps[1], Step::Note(text) if text.contains("Add Widgets")));
    }

    #[test]
    fn a_missing_payload_says_which_directory_was_expected() {
        let root = tempfile::tempdir().unwrap();
        let payload = root.path().join("integrations");
        let error = steps(&payload, &dirs(root.path())).unwrap_err().to_string();
        assert!(error.contains("no Plasma applet at"), "{error}");
        assert!(error.contains("--payload-dir"), "{error}");
    }
}
