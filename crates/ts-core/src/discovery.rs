//! Locate CLI binaries and per-user directories.
//!
//! systemd user services and D-Bus activation start the daemon with a minimal
//! `PATH`, so besides `PATH` we probe the usual per-user install locations
//! (Nix profiles, JavaScript package-manager globals, `~/.local/bin`).

use std::env;
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

fn well_known_dirs(home: &Path, user: Option<&str>) -> Vec<PathBuf> {
    let per_user_profile = user.map(|u| PathBuf::from(format!("/etc/profiles/per-user/{u}/bin")));
    per_user_profile
        .into_iter()
        .chain(HOME_RELATIVE_DIRS.iter().map(|rel| home.join(rel)))
        .chain(SYSTEM_DIRS.iter().map(PathBuf::from))
        .collect()
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
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
pub fn find_binary(name: &str, override_path: &str) -> Option<PathBuf> {
    find_binary_in(name, override_path, &SearchEnv::current())
}

/// Expand a leading `~/` against `home`.
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
