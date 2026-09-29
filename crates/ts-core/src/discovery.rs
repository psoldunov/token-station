//! Locate CLI binaries and per-user directories, and build the `PATH` their
//! children need.
//!
//! systemd user services, D-Bus activation and macOS apps launched from Finder
//! all start the daemon with a minimal `PATH` (`/usr/bin:/bin:/usr/sbin:/sbin`
//! on macOS), so besides `PATH` we probe the usual per-user install locations
//! (Nix profiles, JavaScript package-manager globals, `~/.local/bin`) and the
//! system ones a package manager writes to.

use std::env;
use std::ffi::{OsStr, OsString};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Per-user directories searched after `PATH`, relative to the home directory.
const HOME_RELATIVE_DIRS: &[&str] = &[
    ".nix-profile/bin",
    ".local/state/nix/profile/bin",
    ".local/bin",
    ".bun/bin",
    concat!(".n", "pm-global/bin"),
    ".volta/bin",
    ".claude/local",
    ".cargo/bin",
];

/// System directories searched last.
const SYSTEM_DIRS: &[&str] = &[
    "/run/current-system/sw/bin",
    "/usr/local/bin",
    "/usr/bin",
    "/bin",
];

/// Extra system directories, searched before [`SYSTEM_DIRS`], on macOS only.
///
/// Both are where a macOS package manager puts the CLIs and neither is ever on
/// a Finder-launched app's `PATH`. They are macOS-only rather than universal so
/// Linux keeps resolving binaries in exactly the order it did before: a Linux
/// box with a `/nix/var/nix/profiles/default/bin` would otherwise start
/// preferring it over `/usr/local/bin`.
#[cfg(target_os = "macos")]
const MACOS_SYSTEM_DIRS: &[&str] = &[
    // Homebrew on Apple silicon.
    "/opt/homebrew/bin",
    // The Nix multi-user default profile: where both the Determinate and the
    // official installers put a macOS `nix profile install`.
    "/nix/var/nix/profiles/default/bin",
];

#[cfg(not(target_os = "macos"))]
const MACOS_SYSTEM_DIRS: &[&str] = &[];

fn well_known_dirs(home: &Path, user: Option<&str>) -> Vec<PathBuf> {
    let per_user_profile = user.map(|u| PathBuf::from(format!("/etc/profiles/per-user/{u}/bin")));
    per_user_profile
        .into_iter()
        .chain(HOME_RELATIVE_DIRS.iter().map(|rel| home.join(rel)))
        .chain(MACOS_SYSTEM_DIRS.iter().map(PathBuf::from))
        .chain(SYSTEM_DIRS.iter().map(PathBuf::from))
        .collect()
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Inputs for [`find_binary_in`], separated from the process environment for tests.
#[derive(Debug, Clone)]
pub struct SearchEnv {
    pub path: Option<String>,
    pub home: PathBuf,
    pub user: Option<String>,
}

impl SearchEnv {
    /// The current process environment.
    #[must_use]
    pub fn current() -> SearchEnv {
        SearchEnv {
            path: env::var("PATH").ok(),
            home: home_dir(),
            user: env::var("USER").ok().or_else(|| env::var("LOGNAME").ok()),
        }
    }
}

/// `$HOME`, falling back to `/` (the daemon never runs without a home).
pub fn home_dir() -> PathBuf {
    env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// Find `name` using `override_path` first (a path, or a bare name searched like
/// `name`), then `PATH`, then well-known per-user and system directories.
pub fn find_binary_in(name: &str, override_path: &str, env: &SearchEnv) -> Option<PathBuf> {
    let wanted = match override_path.trim() {
        "" => name,
        other => other,
    };
    if wanted.contains('/') {
        let p = PathBuf::from(wanted);
        return is_executable(&p).then_some(p);
    }
    env.path
        .as_deref()
        .unwrap_or_default()
        .split(':')
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .chain(well_known_dirs(&env.home, env.user.as_deref()))
        .map(|dir| dir.join(wanted))
        .find(|candidate| is_executable(candidate))
}

/// [`find_binary_in`] with the current process environment.
#[must_use]
pub fn find_binary(name: &str, override_path: &str) -> Option<PathBuf> {
    find_binary_in(name, override_path, &SearchEnv::current())
}

/// The `PATH` to give a child that runs `binary`.
///
/// Finding the CLI is only half the problem. A JavaScript package manager
/// installs `codex` and `claude` as scripts starting `#!/usr/bin/env node`, so
/// running one runs `node` — and the daemon's own `PATH` is whatever launched
/// it, which on macOS is Finder's `/usr/bin:/bin:/usr/sbin:/sbin` and on Linux
/// is a systemd user service's equally short one. Neither holds `node`, and the
/// child then dies on its shebang line with a failure that looks nothing like
/// the missing interpreter it is.
///
/// The answer is the process `PATH` first, then the binary's own directory —
/// the interpreter such an install needs is very often its neighbour — then the
/// same well-known directories [`find_binary_in`] searches. Existing entries
/// keep their places and their order, so nothing that already resolves resolves
/// differently; this only adds places to look once those have come up empty.
#[must_use]
pub fn child_path_in(binary: &Path, env: &SearchEnv) -> OsString {
    let inherited = env
        .path
        .as_deref()
        .unwrap_or_default()
        .split(':')
        .filter(|entry| !entry.is_empty())
        .map(PathBuf::from);
    let own_dir = binary
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(Path::to_path_buf);
    join_unique(
        inherited
            .chain(own_dir)
            .chain(well_known_dirs(&env.home, env.user.as_deref())),
    )
}

/// [`child_path_in`] with the current process environment.
#[must_use]
pub fn child_path(binary: &Path) -> OsString {
    child_path_in(binary, &SearchEnv::current())
}

/// Join `dirs` with `:`, keeping the first occurrence of each.
///
/// A linear scan rather than a set: the list is a dozen entries long and has to
/// stay in the order it was built in, which is the whole point of it.
fn join_unique(dirs: impl IntoIterator<Item = PathBuf>) -> OsString {
    let mut seen: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        if !seen.contains(&dir) {
            seen.push(dir);
        }
    }
    let mut joined = OsString::new();
    for (index, dir) in seen.iter().enumerate() {
        if index > 0 {
            joined.push(OsStr::new(":"));
        }
        joined.push(dir);
    }
    joined
}

/// Expand a leading `~/` against `home`.
#[must_use]
pub fn expand_tilde(path: &str, home: &Path) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if path == "~" => home.to_path_buf(),
        None => PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn make_exe(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn child_path_entries(binary: &str, env: &SearchEnv) -> Vec<String> {
        child_path_in(Path::new(binary), env)
            .to_string_lossy()
            .split(':')
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn a_child_keeps_the_inherited_path_first_and_in_order() {
        let env = SearchEnv {
            path: Some("/usr/bin:/bin".into()),
            home: PathBuf::from("/home/u"),
            user: None,
        };
        let entries = child_path_entries("/home/u/.local/bin/codex", &env);
        // Whatever already resolved has to go on resolving the same way.
        assert_eq!(entries[0], "/usr/bin");
        assert_eq!(entries[1], "/bin");
        // Then the binary's own directory: the interpreter it needs is very
        // often the thing installed next to it.
        assert_eq!(entries[2], "/home/u/.local/bin");
        // Then the rest of the places a binary is looked for.
        assert!(entries.contains(&"/home/u/.nix-profile/bin".to_string()));
        assert!(entries.contains(&"/home/u/.bun/bin".to_string()));
        assert!(entries.contains(&"/usr/local/bin".to_string()));
    }

    #[test]
    fn a_child_path_repeats_no_entry() {
        let env = SearchEnv {
            // `/usr/bin` is both inherited and well-known; `.local/bin` is both
            // inherited and the binary's own directory.
            path: Some("/usr/bin:/home/u/.local/bin:/usr/bin".into()),
            home: PathBuf::from("/home/u"),
            user: Some("u".into()),
        };
        let entries = child_path_entries("/home/u/.local/bin/claude", &env);
        let mut unique = entries.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), entries.len(), "{entries:?}");
        assert_eq!(entries[0], "/usr/bin");
        assert_eq!(entries[1], "/home/u/.local/bin");
        assert!(entries.contains(&"/etc/profiles/per-user/u/bin".to_string()));
    }

    #[test]
    fn a_child_path_is_built_even_with_nothing_inherited() {
        let env = SearchEnv {
            path: None,
            home: PathBuf::from("/home/u"),
            user: None,
        };
        let entries = child_path_entries("/opt/tools/codex", &env);
        assert_eq!(entries[0], "/opt/tools");
        assert!(entries.contains(&"/usr/bin".to_string()));
        assert!(!entries.iter().any(String::is_empty), "{entries:?}");

        // A bare name has no directory of its own to add, and that is not an
        // empty entry.
        let bare = child_path_entries("codex", &env);
        assert!(!bare.iter().any(String::is_empty), "{bare:?}");
        assert_eq!(bare[0], "/home/u/.nix-profile/bin");
    }

    #[test]
    fn the_extra_system_dirs_are_macos_only_and_come_first() {
        let dirs = well_known_dirs(Path::new("/home/u"), None);
        let position = |wanted: &str| dirs.iter().position(|d| d == Path::new(wanted));

        assert_eq!(
            position("/opt/homebrew/bin").is_some(),
            cfg!(target_os = "macos")
        );
        assert_eq!(
            position("/nix/var/nix/profiles/default/bin").is_some(),
            cfg!(target_os = "macos")
        );

        // Wherever they are searched, the order of the ones Linux already had
        // is untouched.
        let system = [
            "/run/current-system/sw/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/bin",
        ];
        let found: Vec<usize> = system.iter().filter_map(|d| position(d)).collect();
        assert_eq!(found.len(), system.len());
        assert!(found.windows(2).all(|pair| pair[0] < pair[1]), "{found:?}");

        #[cfg(target_os = "macos")]
        {
            // Before the system ones: a Homebrew `claude` is the user's own
            // install and beats anything a base system ships.
            assert!(position("/opt/homebrew/bin") < position("/usr/local/bin"));
        }
    }

    #[test]
    fn finds_on_path_before_well_known_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let bin = tmp.path().join("bin");
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        fs::create_dir_all(&bin).unwrap();
        let local = make_exe(&home.join(".local/bin"), "tscodexprobe");
        let env = SearchEnv {
            path: Some(bin.display().to_string()),
            home: home.clone(),
            user: None,
        };
        assert_eq!(find_binary_in("tscodexprobe", "", &env), Some(local));
        let on_path = make_exe(&bin, "tscodexprobe");
        assert_eq!(find_binary_in("tscodexprobe", "", &env), Some(on_path));
        assert_eq!(find_binary_in("definitely-missing-tool", "", &env), None);
    }

    #[test]
    fn override_path_wins_and_must_be_executable() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = make_exe(tmp.path(), "my-claude");
        let env = SearchEnv {
            path: None,
            home: tmp.path().to_path_buf(),
            user: None,
        };
        let o = exe.display().to_string();
        assert_eq!(find_binary_in("claude", &o, &env), Some(exe));
        let plain = tmp.path().join("plain");
        fs::write(&plain, "x").unwrap();
        assert_eq!(
            find_binary_in("claude", &plain.display().to_string(), &env),
            None
        );
    }

    #[test]
    fn expands_tilde() {
        let home = Path::new("/home/u");
        assert_eq!(
            expand_tilde("~/.codex", home),
            PathBuf::from("/home/u/.codex")
        );
        assert_eq!(expand_tilde("~", home), PathBuf::from("/home/u"));
        assert_eq!(expand_tilde("/abs", home), PathBuf::from("/abs"));
    }
}
