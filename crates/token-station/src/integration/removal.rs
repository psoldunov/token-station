//! Which recorded paths `uninstall` is actually allowed to delete.
//!
//! The manifest is a plain JSON file in the state directory, so anything that can
//! write there can ask for a path to be removed. The list is therefore checked
//! rather than trusted: only the fixed names `setup` installs, only under the XDG
//! roots it writes into, and only when no symlink along the way leaves those roots.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use crate::integration::claude_settings;
use crate::integration::manifest::ClaudeRecord;
use crate::integration::{Dirs, dbus_activation, desktop, gnome, plasma, systemd};

/// The only file name `--claude-statusline` ever patches.
pub const CLAUDE_SETTINGS: &str = "settings.json";

/// Directory names `setup` creates and owns outright.
#[must_use]
pub fn dir_names() -> Vec<&'static str> {
    vec![plasma::APPLET_ID, gnome::EXTENSION_UUID]
}

/// File names `setup` writes.
#[must_use]
pub fn file_names() -> Vec<&'static str> {
    let mut names = vec![
        desktop::ENTRY,
        desktop::TRAY_ENTRY,
        dbus_activation::SERVICE_FILE,
        systemd::UNIT,
    ];
    names.extend(desktop::ICON_FILES);
    names
}

/// The roots `setup` is allowed to write into.
fn roots(dirs: &Dirs) -> Vec<PathBuf> {
    vec![dirs.data.clone(), dirs.config.clone(), dirs.state.clone()]
}

/// Is `path` a directory `setup` creates, inside a root it writes into?
#[must_use]
pub fn removable_dir(path: &Path, dirs: &Dirs) -> bool {
    named_under(path, &roots(dirs), &dir_names())
}

/// Is `path` a file `setup` writes, or the one backup it took?
///
/// The backup sits next to Claude Code's `settings.json`, which the user may put
/// anywhere, so it is allowed by exact match against the record being undone —
/// and only after [`trusted_claude_record`] has agreed that record is one of ours.
#[must_use]
pub fn removable_file(path: &Path, dirs: &Dirs, backup: Option<&Path>) -> bool {
    backup.is_some_and(|known| known == path) || named_under(path, &roots(dirs), &file_names())
}

/// Is `record` a statusline record `uninstall` may act on?
///
/// The manifest is a plain JSON file in the state directory, so a `claude` record
/// naming `~/.ssh/id_ed25519` as its "backup" is a file anything that can write
/// there could ask `uninstall` to delete, and any JSON file at all could be asked
/// to have its `statusLine` rewritten. Neither is trusted: the settings file has
/// to be the `settings.json` in the Claude config directory this session resolves
/// to, and the backup has to be exactly the path
/// [`claude_settings::backup_path`] derives from it.
#[must_use]
pub fn trusted_claude_record(record: &ClaudeRecord, settings: &Path) -> bool {
    record.settings == settings
        && record.settings.file_name() == Some(OsStr::new(CLAUDE_SETTINGS))
        && record.backup == claude_settings::backup_path(settings)
}

/// `path` carries one of `names`, sits under one of `roots`, and gets there
/// without `..` or a symlink that leads out again.
fn named_under(path: &Path, roots: &[PathBuf], names: &[&str]) -> bool {
    if !path.is_absolute() || path.components().any(|c| c == Component::ParentDir) {
        return false;
    }
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if !names.contains(&name) {
        return false;
    }
    let Some(parent) = path.parent() else {
        return false;
    };
    // A parent that is not there cannot hold anything worth deleting.
    let Ok(real_parent) = parent.canonicalize() else {
        return false;
    };
    roots.iter().any(|root| {
        let real_root = root.canonicalize().unwrap_or_else(|_| root.clone());
        path.starts_with(root) && real_parent.starts_with(&real_root)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Home {
        _dir: tempfile::TempDir,
        dirs: Dirs,
    }

    fn home() -> Home {
        let dir = tempfile::tempdir().expect("temp dir");
        let at = |suffix: &str| {
            let path = dir.path().join(suffix);
            std::fs::create_dir_all(&path).expect("root created");
            path
        };
        let dirs = Dirs {
            home: dir.path().to_path_buf(),
            data: at("data"),
            config: at("config"),
            state: at("state"),
        };
        Home { _dir: dir, dirs }
    }

    /// Create `relative` under `root` and hand back its path.
    fn touch(root: &Path, relative: &str) -> PathBuf {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("parents created");
        std::fs::write(&path, "x").expect("file written");
        path
    }

    #[test]
    fn the_files_setup_writes_are_removable() {
        let home = home();
        for relative in [
            "applications/dev.soldunov.TokenStation.desktop",
            "dbus-1/services/dev.soldunov.TokenStation.service",
            "icons/hicolor/scalable/apps/dev.soldunov.TokenStation.svg",
            "icons/hicolor/symbolic/apps/dev.soldunov.TokenStation-symbolic.svg",
        ] {
            let path = touch(&home.dirs.data, relative);
            assert!(
                removable_file(&path, &home.dirs, None),
                "{} should be removable",
                path.display()
            );
        }
        for relative in [
            "systemd/user/token-station.service",
            "autostart/dev.soldunov.TokenStation.Tray.desktop",
        ] {
            let path = touch(&home.dirs.config, relative);
            assert!(removable_file(&path, &home.dirs, None), "{relative}");
        }
    }

    #[test]
    fn the_directories_setup_owns_are_removable() {
        let home = home();
        for (root, relative) in [
            (
                &home.dirs.data,
                "plasma/plasmoids/dev.soldunov.tokenstation",
            ),
            (
                &home.dirs.data,
                "gnome-shell/extensions/token-station@soldunov.dev",
            ),
        ] {
            let path = root.join(relative);
            std::fs::create_dir_all(&path).unwrap();
            assert!(removable_dir(&path, &home.dirs), "{relative}");
        }
    }

    #[test]
    fn a_tampered_manifest_cannot_reach_the_home_directory() {
        let home = home();
        let hostage = touch(&home.dirs.home, ".bashrc");
        assert!(!removable_file(&hostage, &home.dirs, None));
        assert!(!removable_dir(&home.dirs.home, &home.dirs));
        assert!(!removable_file(Path::new("/etc/passwd"), &home.dirs, None));
        assert!(!removable_dir(Path::new("/"), &home.dirs));
        // Right name, wrong place.
        let elsewhere = touch(&home.dirs.home, "token-station.service");
        assert!(!removable_file(&elsewhere, &home.dirs, None));
    }

    #[test]
    fn a_name_setup_never_writes_is_refused() {
        let home = home();
        let stranger = touch(&home.dirs.data, "applications/someone-else.desktop");
        assert!(!removable_file(&stranger, &home.dirs, None));
        let relative = Path::new("data/applications/dev.soldunov.TokenStation.desktop");
        assert!(!removable_file(relative, &home.dirs, None), "not absolute");
    }

    #[test]
    fn a_traversal_or_a_symlink_out_of_the_roots_is_refused() {
        let home = home();
        let escape = home
            .dirs
            .data
            .join("applications/../../token-station.service");
        assert!(
            !removable_file(&escape, &home.dirs, None),
            "`..` is refused"
        );

        // A directory inside the root that actually lives outside it.
        let outside = home.dirs.home.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let linked = home.dirs.data.join("applications");
        std::os::unix::fs::symlink(&outside, &linked).unwrap();
        let target = linked.join(desktop::ENTRY);
        std::fs::write(&target, "x").unwrap();
        assert!(
            !removable_file(&target, &home.dirs, None),
            "a symlinked parent that leaves the root is refused"
        );
    }

    /// A record for `settings`, with the backup name `setup` really writes.
    fn record(settings: &Path) -> ClaudeRecord {
        ClaudeRecord {
            settings: settings.to_path_buf(),
            backup: claude_settings::backup_path(settings),
            previous: None,
            command: "'/opt/TokenStation.AppImage' statusline".into(),
        }
    }

    #[test]
    fn only_the_settings_file_this_session_resolves_to_is_trusted() {
        let home = home();
        let settings = home.dirs.home.join(".claude/settings.json");
        assert!(trusted_claude_record(&record(&settings), &settings));

        // A manifest naming somebody else's file, or another directory's.
        let elsewhere = home.dirs.home.join(".claude/config.json");
        assert!(!trusted_claude_record(&record(&elsewhere), &elsewhere));
        let other_dir = home.dirs.home.join(".config/claude/settings.json");
        assert!(!trusted_claude_record(&record(&other_dir), &settings));

        // And a backup that is not the settings file plus the suffix.
        let mut tampered = record(&settings);
        tampered.backup = home.dirs.home.join(".ssh/id_ed25519");
        assert!(!trusted_claude_record(&tampered, &settings));
    }

    #[test]
    fn the_recorded_backup_is_removable_by_exact_path() {
        let home = home();
        let backup = touch(
            &home.dirs.home,
            ".claude/settings.json.token-station-backup",
        );
        assert!(!removable_file(&backup, &home.dirs, None));
        assert!(removable_file(&backup, &home.dirs, Some(&backup)));
        // And only that one path.
        let other = home.dirs.home.join(".claude/settings.json");
        assert!(!removable_file(&other, &home.dirs, Some(&backup)));
    }
}
