//! The Plasma applet: a plasmoid directory copied into the user's data home.
//!
//! Plasma's system tray lists applets once, when plasmashell starts. After that
//! it only adds, restarts or drops one when `KPackage` announces the change on
//! the session bus, so `setup` and `uninstall` send that announcement
//! themselves: a copy alone would not show up until the next login.

use std::path::{Path, PathBuf};

use crate::dbus::client::with_session_bus_blocking;
use crate::integration::Dirs;
use crate::integration::step::Step;

/// Directory name of the applet, inside the payload and in the data home.
pub const APPLET_ID: &str = "dev.soldunov.tokenstation";
/// Name of the payload directory holding the applet.
pub const PAYLOAD: &str = "plasma";
/// Object path `KPackage` announces Plasma applet changes on.
pub const KPACKAGE_PATH: &str = "/KPackage/Plasma/Applet";
/// Interface of those announcements.
pub const KPACKAGE_INTERFACE: &str = "org.kde.plasma.kpackage";

/// What happened to the applet, in `KPackage`'s terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Installed,
    Uninstalled,
}

impl Change {
    /// The signal `KPackage` sends for this change.
    #[must_use]
    pub fn signal(self) -> &'static str {
        match self {
            Change::Installed => "packageInstalled",
            Change::Uninstalled => "packageUninstalled",
        }
    }
}

/// A `packageInstalled` announcement for the applet at `applet`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    pub applet: PathBuf,
    /// Which bus to announce on; `None` means `$DBUS_SESSION_BUS_ADDRESS`.
    pub bus_address: Option<String>,
}

impl Announcement {
    /// Send it, but only when this run `installed` the applet: announcing an
    /// applet somebody else put there would restart theirs. A failure comes
    /// back as a warning.
    #[must_use]
    pub fn send(&self, installed: &[PathBuf]) -> Option<String> {
        if !installed.contains(&self.applet) {
            return None;
        }
        announce_or_warn(self.bus_address.as_deref(), Change::Installed)
    }
}

/// Where the applet is installed.
#[must_use]
pub fn target(dirs: &Dirs) -> PathBuf {
    dirs.data.join("plasma/plasmoids").join(APPLET_ID)
}

/// Copy the applet out of `payload` (the `integrations` directory) and tell a
/// running plasmashell about it on `bus_address` (`None`: the session bus).
///
/// # Errors
///
/// Returns an error when `payload` holds no Plasma applet directory, which means
/// the caller needs `--payload-dir` or the `AppImage`.
pub fn steps(payload: &Path, dirs: &Dirs, bus_address: Option<&str>) -> anyhow::Result<Vec<Step>> {
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
        Step::AnnouncePlasmoid(Announcement {
            applet: target.clone(),
            bus_address: bus_address.map(str::to_string),
        }),
        Step::Note(format!(
            "Plasma applet installed at {}; it joins the system tray by itself. If it \
             stays hidden, tick it under System Tray Settings → Entries.",
            target.display()
        )),
    ])
}

/// Send `KPackage`'s `change` signal for the applet on `bus_address`.
///
/// A signal needs no listener, so this succeeds on any reachable bus, Plasma
/// or not.
///
/// # Errors
///
/// Returns a message when the bus cannot be reached or the signal not sent.
pub fn announce(bus_address: Option<&str>, change: Change) -> Result<(), String> {
    with_session_bus_blocking(bus_address, |connection| async move {
        connection
            .emit_signal(
                None::<zbus::names::BusName<'_>>,
                KPACKAGE_PATH,
                KPACKAGE_INTERFACE,
                change.signal(),
                &APPLET_ID,
            )
            .await
    })
}

/// Tell a running plasmashell the applet is gone, when `recorded` (the
/// manifest's directories) held it and it no longer exists. A failure comes
/// back as a warning.
#[must_use]
pub fn announce_removal(recorded: &[PathBuf], bus_address: Option<&str>) -> Option<String> {
    let installed_at = Path::new("plasma/plasmoids").join(APPLET_ID);
    let removed = recorded
        .iter()
        .any(|dir| dir.ends_with(&installed_at) && std::fs::symlink_metadata(dir).is_err());
    if !removed {
        return None;
    }
    announce_or_warn(bus_address, Change::Uninstalled)
}

/// [`announce`], as a warning when it fails: the applet still arrives or
/// leaves at the next login, so nothing else has to stop.
#[must_use]
pub fn announce_or_warn(bus_address: Option<&str>, change: Change) -> Option<String> {
    let verb = match change {
        Change::Installed => "joins",
        Change::Uninstalled => "leaves",
    };
    announce(bus_address, change).err().map(|error| {
        format!(
            "cannot tell Plasma about the applet ({error}); it {verb} the system tray at \
             the next login"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integration::existing::Previous;
    use crate::integration::manifest::Manifest;
    use crate::integration::step::{self, Outcome};

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
    fn a_present_payload_produces_a_copy_an_announcement_and_a_hint() {
        let root = tempfile::tempdir().unwrap();
        let payload = root.path().join("integrations");
        std::fs::create_dir_all(payload.join(PAYLOAD)).unwrap();
        let dirs = dirs(root.path());

        let steps = steps(&payload, &dirs, Some(UNREACHABLE_BUS)).unwrap();
        assert_eq!(
            steps[0],
            Step::CopyTree {
                source: payload.join(PAYLOAD),
                target: target(&dirs),
            }
        );
        // The copy alone does not reach a running plasmashell: its tray only
        // rescans when KPackage says so on the session bus.
        assert_eq!(
            steps[1],
            Step::AnnouncePlasmoid(Announcement {
                applet: target(&dirs),
                bus_address: Some(UNREACHABLE_BUS.into()),
            })
        );
        // The applet is EnabledByDefault with a notification-area category, so it
        // appears on its own: telling the user to add a widget would be wrong.
        let Step::Note(text) = &steps[2] else {
            panic!("expected a note, got {:?}", steps[2])
        };
        assert!(text.contains("joins the system tray by itself"), "{text}");
        assert!(text.contains("System Tray Settings"), "{text}");
        assert!(!text.contains("Add Widgets"), "{text}");
    }

    #[test]
    fn a_missing_payload_says_which_directory_was_expected() {
        let root = tempfile::tempdir().unwrap();
        let payload = root.path().join("integrations");
        let error = steps(&payload, &dirs(root.path()), None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no Plasma applet at"), "{error}");
        assert!(error.contains("--payload-dir"), "{error}");
    }

    /// A bus that is never there, so no test reaches the developer's session.
    const UNREACHABLE_BUS: &str = "unix:path=/nonexistent/token-station-test";

    /// Run the install steps against a payload holding a minimal applet.
    fn install(root: &Path) -> (Outcome, Manifest) {
        let payload = root.join("integrations");
        std::fs::create_dir_all(payload.join(PAYLOAD)).unwrap();
        std::fs::write(payload.join(PAYLOAD).join("metadata.json"), "{}").unwrap();
        let mut manifest = Manifest::new(Path::new("/opt/x.AppImage"), "kde", 0);
        let plan = steps(&payload, &dirs(root), Some(UNREACHABLE_BUS)).unwrap();
        let outcome = step::execute(&plan, &mut manifest, Previous::default(), None).unwrap();
        (outcome, manifest)
    }

    #[test]
    fn an_unreachable_bus_leaves_the_applet_for_the_next_login() {
        let root = tempfile::tempdir().unwrap();
        let (outcome, manifest) = install(root.path());

        assert_eq!(manifest.dirs, vec![target(&dirs(root.path()))]);
        assert_eq!(outcome.warnings.len(), 1, "{outcome:?}");
        assert!(outcome.warnings[0].contains("Plasma"), "{outcome:?}");
        assert!(outcome.warnings[0].contains("next login"), "{outcome:?}");
    }

    #[test]
    fn an_applet_this_run_did_not_install_is_not_announced() {
        let root = tempfile::tempdir().unwrap();
        let theirs = target(&dirs(root.path()));
        std::fs::create_dir_all(&theirs).unwrap();
        std::fs::write(theirs.join("theirs.qml"), "mine").unwrap();

        let (outcome, manifest) = install(root.path());

        assert!(manifest.dirs.is_empty());
        // Only the refusal to overwrite: announcing would restart their applet.
        assert_eq!(outcome.warnings.len(), 1, "{outcome:?}");
        assert!(outcome.warnings[0].contains("--force"), "{outcome:?}");
    }

    #[test]
    fn only_an_applet_that_is_gone_is_announced_as_removed() {
        let root = tempfile::tempdir().unwrap();
        let applet = target(&dirs(root.path()));
        std::fs::create_dir_all(&applet).unwrap();
        let recorded = vec![applet.clone()];

        // Still there (removal refused or failed): the tray must keep it.
        assert_eq!(announce_removal(&recorded, Some(UNREACHABLE_BUS)), None);
        // Never ours: nothing to say.
        std::fs::remove_dir_all(&applet).unwrap();
        assert_eq!(announce_removal(&[], Some(UNREACHABLE_BUS)), None);

        let warning = announce_removal(&recorded, Some(UNREACHABLE_BUS)).expect("announced");
        assert!(warning.contains("leaves the system tray"), "{warning}");
        assert!(warning.contains("next login"), "{warning}");
    }

    #[test]
    fn each_change_is_the_signal_kpackage_itself_sends() {
        assert_eq!(Change::Installed.signal(), "packageInstalled");
        assert_eq!(Change::Uninstalled.signal(), "packageUninstalled");
    }
}
