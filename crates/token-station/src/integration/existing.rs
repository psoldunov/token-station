//! What is already on disk where `setup` wants to write.
//!
//! An `AppImage` install must never quietly replace a file it does not own. A
//! home-manager generation puts symlinks into `/nix/store` exactly where the
//! `.desktop` entry, the unit and the activation file go: renaming over one of
//! those replaces the link, the manifest then claims it as ours, and `uninstall`
//! deletes a file the user's configuration owns. So an unexpected target is left
//! alone and reported, unless `--force` says otherwise — and a `/nix/store` link is
//! left alone either way, because nothing good comes of shadowing a package.

use std::path::{Path, PathBuf};

use crate::integration::manifest::ClaudeRecord;

/// The Nix store: anything linked into it belongs to a package, not to `setup`.
pub const NIX_STORE: &str = "/nix/store";

/// Directories a packaged systemd user unit can come from.
pub const SYSTEM_UNIT_DIRS: [&str; 3] = [
    "/etc/systemd/user",
    "/run/systemd/user",
    "/usr/lib/systemd/user",
];
/// Directories a packaged D-Bus session service file can come from.
pub const SYSTEM_DBUS_DIRS: [&str; 2] = ["/usr/share/dbus-1/services", "/etc/dbus-1/services"];
/// Per-user Nix profiles, relative to `$HOME`.
const NIX_PROFILES: [&str; 2] = [".nix-profile/share", ".local/state/nix/profile/share"];

/// The system-wide roots searched for a packaged copy of what `setup` installs.
///
/// Held as data rather than read from the constants directly so a test can point
/// them at a temp directory: whether this machine happens to carry a packaged unit
/// in `/usr/lib` must not decide what a test observes. `Default` is therefore
/// empty — nothing packaged — and only [`SystemDirs::current`] reads the real ones.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemDirs {
    pub units: Vec<PathBuf>,
    pub dbus: Vec<PathBuf>,
}

impl SystemDirs {
    /// The real directories a distribution or Nix package writes into.
    pub fn current() -> SystemDirs {
        SystemDirs {
            units: SYSTEM_UNIT_DIRS.iter().map(PathBuf::from).collect(),
            dbus: SYSTEM_DBUS_DIRS.iter().map(PathBuf::from).collect(),
        }
    }
}

/// What an earlier `setup` left behind, and how forceful this run may be.
#[derive(Debug, Clone, Copy, Default)]
pub struct Previous<'a> {
    /// Paths the last manifest recorded, which a re-run may rewrite.
    pub owned: &'a [PathBuf],
    /// The statusline record from that run, whose `previous` value must survive.
    pub claude: Option<&'a ClaudeRecord>,
    /// `--force`: replace files `setup` did not install.
    pub force: bool,
}

/// What `setup` may do with one target path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing is in the way, or an earlier run put it there.
    Write,
    /// Leave it alone; the text says why, for the summary.
    Skip(String),
}

/// Decide what to do about `target`.
#[must_use]
pub fn verdict(target: &Path, previous: Previous<'_>) -> Verdict {
    let Ok(meta) = std::fs::symlink_metadata(target) else {
        return Verdict::Write;
    };
    let ours = previous.owned.iter().any(|known| known == target);
    if meta.file_type().is_symlink() {
        return symlink_verdict(target, ours || previous.force);
    }
    if ours || previous.force {
        return Verdict::Write;
    }
    Verdict::Skip(format!(
        "{} already exists and Token Station did not install it; left it alone \
         (re-run with --force to replace it)",
        target.display()
    ))
}

/// A link into the store is never replaced; any other link needs permission.
fn symlink_verdict(target: &Path, allowed: bool) -> Verdict {
    let into_store = std::fs::read_link(target).is_ok_and(|to| to.starts_with(NIX_STORE));
    if into_store {
        return Verdict::Skip(format!(
            "{} is a symlink into {NIX_STORE}, so Nix or home-manager owns it; \
             left it alone and changed nothing",
            target.display()
        ));
    }
    if allowed {
        return Verdict::Write;
    }
    Verdict::Skip(format!(
        "{} is a symlink Token Station did not create; left it alone \
         (re-run with --force to replace it)",
        target.display()
    ))
}

/// The first of `candidates` that exists.
#[must_use]
pub fn packaged(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|path| std::fs::symlink_metadata(path).is_ok())
        .cloned()
}

/// Where a packaged systemd user unit called `unit` could already be.
#[must_use]
pub fn unit_candidates(system: &SystemDirs, home: &Path, unit: &str) -> Vec<PathBuf> {
    candidates(&system.units, home, "systemd/user", unit)
}

/// Where a packaged D-Bus activation file called `file` could already be.
#[must_use]
pub fn service_candidates(system: &SystemDirs, home: &Path, file: &str) -> Vec<PathBuf> {
    candidates(&system.dbus, home, "dbus-1/services", file)
}

fn candidates(system: &[PathBuf], home: &Path, suffix: &str, name: &str) -> Vec<PathBuf> {
    system
        .iter()
        .map(|dir| dir.join(name))
        .chain(
            NIX_PROFILES
                .iter()
                .map(|profile| home.join(profile).join(suffix).join(name)),
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Previous` that recorded `owned` and may or may not be forceful.
    fn previous(owned: &[PathBuf], force: bool) -> Previous<'_> {
        Previous {
            owned,
            claude: None,
            force,
        }
    }

    #[test]
    fn who_put_the_file_there_decides_whether_it_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let absent = dir.path().join("nothing-here.desktop");
        let ours = dir.path().join("ours.desktop");
        let theirs = dir.path().join("theirs.desktop");
        for path in [&ours, &theirs] {
            std::fs::write(path, "[Desktop Entry]").expect("file written");
        }
        let recorded = vec![ours.clone()];

        // Nothing there, or a file the last run installed: write it.
        assert_eq!(verdict(&absent, previous(&[], false)), Verdict::Write);
        assert_eq!(verdict(&ours, previous(&recorded, false)), Verdict::Write);

        // Somebody else's file: left alone, and the reason says how to override.
        let Verdict::Skip(why) = verdict(&theirs, previous(&[], false)) else {
            panic!("a file we never installed must not be replaced");
        };
        assert!(why.contains("--force"), "{why}");
        assert_eq!(verdict(&theirs, previous(&[], true)), Verdict::Write);
    }

    #[test]
    fn a_store_symlink_is_never_replaced_even_with_force() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("managed.service");
        // The link need not resolve: what matters is where it points.
        std::os::unix::fs::symlink("/nix/store/abc123-token-station/x.service", &target).unwrap();

        for forceful in [false, true] {
            let Verdict::Skip(why) = verdict(&target, previous(&[], forceful)) else {
                panic!("a /nix/store link must never be replaced");
            };
            assert!(why.contains(NIX_STORE), "{why}");
        }
    }

    #[test]
    fn an_ordinary_symlink_needs_ownership_or_force() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("dotfiles.desktop");
        std::fs::write(&real, "[Desktop Entry]").unwrap();
        let target = dir.path().join("linked.desktop");
        std::os::unix::fs::symlink(&real, &target).unwrap();

        assert!(matches!(
            verdict(&target, previous(&[], false)),
            Verdict::Skip(_)
        ));
        assert_eq!(verdict(&target, previous(&[], true)), Verdict::Write);
    }

    #[test]
    fn a_packaged_copy_is_found_in_the_system_and_profile_directories() {
        let home = Path::new("/home/u");
        let system = SystemDirs::current();
        let units = unit_candidates(&system, home, "token-station.service");
        assert!(units.contains(&PathBuf::from("/etc/systemd/user/token-station.service")));
        assert!(units.contains(&PathBuf::from(
            "/home/u/.nix-profile/share/systemd/user/token-station.service"
        )));

        let services = service_candidates(&system, home, "dev.soldunov.TokenStation.service");
        assert!(services.contains(&PathBuf::from(
            "/usr/share/dbus-1/services/dev.soldunov.TokenStation.service"
        )));
        assert!(services.contains(&PathBuf::from(
            "/home/u/.local/state/nix/profile/share/dbus-1/services/dev.soldunov.TokenStation.service"
        )));
    }

    #[test]
    fn packaged_reports_the_first_candidate_that_is_there() {
        let dir = tempfile::tempdir().unwrap();
        let absent = dir.path().join("absent");
        let present = dir.path().join("present");
        std::fs::write(&present, "x").unwrap();
        assert_eq!(packaged(std::slice::from_ref(&absent)), None);
        assert_eq!(packaged(&[absent, present.clone()]), Some(present));
        assert_eq!(packaged(&[]), None);
    }

    #[test]
    fn the_search_roots_can_be_pointed_somewhere_else() {
        let dir = tempfile::tempdir().unwrap();
        let system = SystemDirs {
            units: vec![dir.path().join("usr/lib/systemd/user")],
            dbus: vec![dir.path().join("usr/share/dbus-1/services")],
        };
        let units = unit_candidates(&system, Path::new("/home/u"), "token-station.service");
        assert_eq!(
            units.first(),
            Some(
                &dir.path()
                    .join("usr/lib/systemd/user/token-station.service")
            )
        );
        // The per-user Nix profiles are still searched, under the given home.
        assert!(units.iter().any(|p| p.starts_with("/home/u/.nix-profile")));
        assert!(
            service_candidates(&system, Path::new("/home/u"), "x.service")
                .iter()
                .all(|p| !p.starts_with("/usr"))
        );
    }
}
