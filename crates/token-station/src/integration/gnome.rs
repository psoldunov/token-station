//! The GNOME Shell extension: an extension directory plus `gnome-extensions enable`.

use std::path::{Path, PathBuf};

use crate::integration::Dirs;
use crate::integration::step::Step;

/// UUID of the extension, which is also its directory name.
pub const EXTENSION_UUID: &str = "token-station@soldunov.dev";
/// Name of the payload directory holding the extension.
pub const PAYLOAD: &str = "gnome";
/// The CLI that enables it, when the session has one.
pub const ENABLE_TOOL: &str = "gnome-extensions";
/// GNOME Shell cannot load a new extension into a running Wayland session.
pub const WAYLAND_NOTE: &str =
    "GNOME on Wayland cannot load a new extension into the running session: log out and back in.";

/// Where the extension is installed.
#[must_use]
pub fn target(dirs: &Dirs) -> PathBuf {
    dirs.data
        .join("gnome-shell/extensions")
        .join(EXTENSION_UUID)
}

/// Copy the extension and, when `enable_tool` exists, enable it.
///
/// # Errors
///
/// Returns an error when `payload` holds no GNOME extension directory, which
/// means the caller needs `--payload-dir` or the `AppImage`.
pub fn steps(payload: &Path, dirs: &Dirs, enable_tool: Option<&Path>) -> anyhow::Result<Vec<Step>> {
    let source = payload.join(PAYLOAD);
    if !source.is_dir() {
        anyhow::bail!(
            "no GNOME extension at {}; pass --payload-dir or use the AppImage",
            source.display()
        );
    }
    let target = target(dirs);
    let mut steps = vec![Step::CopyTree {
        source,
        target: target.clone(),
    }];
    match enable_tool {
        Some(tool) => steps.push(Step::Run {
            program: tool.to_path_buf(),
            args: vec!["enable".into(), EXTENSION_UUID.into()],
        }),
        None => steps.push(Step::Note(format!(
            "`{ENABLE_TOOL}` was not found: enable \"{EXTENSION_UUID}\" from the Extensions app."
        ))),
    }
    steps.push(Step::Note(WAYLAND_NOTE.into()));
    Ok(steps)
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

    fn payload(root: &Path) -> PathBuf {
        let payload = root.join("integrations");
        std::fs::create_dir_all(payload.join(PAYLOAD)).unwrap();
        payload
    }

    #[test]
    fn the_extension_lands_where_gnome_shell_looks() {
        let dirs = Dirs {
            home: "/home/u".into(),
            data: "/da".into(),
            config: "/cfg".into(),
            state: "/st".into(),
        };
        assert_eq!(
            target(&dirs),
            Path::new("/da/gnome-shell/extensions/token-station@soldunov.dev")
        );
    }

    #[test]
    fn with_the_cli_present_the_extension_is_enabled() {
        let root = tempfile::tempdir().unwrap();
        let dirs = dirs(root.path());
        let tool = PathBuf::from("/usr/bin/gnome-extensions");
        let steps = steps(&payload(root.path()), &dirs, Some(&tool)).unwrap();

        assert!(matches!(&steps[0], Step::CopyTree { target: t, .. } if *t == target(&dirs)));
        assert_eq!(
            steps[1],
            Step::Run {
                program: tool,
                args: vec!["enable".into(), EXTENSION_UUID.into()],
            }
        );
        assert_eq!(steps[2], Step::Note(WAYLAND_NOTE.into()));
    }

    #[test]
    fn without_the_cli_the_user_is_told_how_to_enable_it() {
        let root = tempfile::tempdir().unwrap();
        let steps = steps(&payload(root.path()), &dirs(root.path()), None).unwrap();
        assert!(!steps.iter().any(|step| matches!(step, Step::Run { .. })));
        assert!(
            matches!(&steps[1], Step::Note(text) if text.contains("Extensions app")),
            "{:?}",
            steps[1]
        );
    }

    #[test]
    fn a_missing_payload_says_which_directory_was_expected() {
        let root = tempfile::tempdir().unwrap();
        let error = steps(&root.path().join("integrations"), &dirs(root.path()), None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no GNOME extension at"), "{error}");
    }
}
