//! Where the daemon keeps its files.
//!
//! On Linux every location follows the XDG base directory spec. On macOS there is
//! no XDG layout and, more to the point, no agreement on it: the menu bar app is
//! launched by Finder, the CLI from a shell and the statusline hook by Claude
//! Code, and each of those three gets a different environment. So macOS ignores
//! the XDG variables entirely and uses the Apple layout, which all three resolve
//! to the same way. The environment is taken as an explicit value so tests can
//! build a [`Paths`] pointing into a temp directory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Directory name used under every XDG root.
pub const APP_DIR: &str = "token-station";

/// Bundle identifier used for the macOS `Application Support` and `Caches` dirs.
pub const MACOS_APP_DIR: &str = "dev.soldunov.TokenStation";

/// Socket file name, inside [`Paths::runtime_dir`].
pub const SOCKET_FILE: &str = "daemon.sock";

/// Extension of the lock file that sits beside the socket.
pub const LOCK_EXTENSION: &str = "lock";

/// Overrides the socket path for every component (daemon, CLI, app).
pub const SOCKET_ENV: &str = "TOKEN_STATION_SOCKET";

/// Longest socket path `sockaddr_un.sun_path` holds, without the terminating NUL.
///
/// 104 bytes on macOS against 108 on Linux; the check is done against the local
/// limit, because a path that binds here is all this process needs.
#[cfg(target_os = "macos")]
pub const MAX_SOCKET_PATH_LEN: usize = 103;
#[cfg(not(target_os = "macos"))]
pub const MAX_SOCKET_PATH_LEN: usize = 107;

/// A socket path `bind(2)` would truncate.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the socket path {path} is {len} bytes, more than the {max} a Unix socket holds")]
pub struct SocketPathTooLong {
    pub path: String,
    pub len: usize,
    pub max: usize,
}

/// Refuse a socket path longer than `max` bytes.
///
/// `bind(2)` copies the path into a fixed `sun_path` array, so a path one byte
/// too long does not fail: it binds a *different*, truncated path, and every
/// client then connects to something that is not there.
///
/// # Errors
///
/// Returns [`SocketPathTooLong`] when `path` does not fit.
pub fn check_socket_path(path: &Path, max: usize) -> Result<(), SocketPathTooLong> {
    let len = path.as_os_str().as_encoded_bytes().len();
    if len > max {
        return Err(SocketPathTooLong {
            path: path.display().to_string(),
            len,
            max,
        });
    }
    Ok(())
}

/// The subset of the environment that decides the paths.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Env {
    pub home: Option<String>,
    pub config_home: Option<String>,
    pub state_home: Option<String>,
    pub cache_home: Option<String>,
    pub runtime_dir: Option<String>,
    /// `$XDG_DATA_HOME`; where `setup` drops applets, extensions and icons.
    pub data_home: Option<String>,
    /// `$TOKEN_STATION_SOCKET`; an explicit socket path for every component.
    pub socket: Option<String>,
}

impl Env {
    /// Read the relevant variables from the process environment.
    #[must_use]
    pub fn current() -> Env {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Env {
            home: var("HOME"),
            config_home: var("XDG_CONFIG_HOME"),
            state_home: var("XDG_STATE_HOME"),
            cache_home: var("XDG_CACHE_HOME"),
            runtime_dir: var("XDG_RUNTIME_DIR"),
            data_home: var("XDG_DATA_HOME"),
            socket: var(SOCKET_ENV),
        }
    }

    /// Build from a map, for tests.
    #[must_use]
    pub fn from_map(vars: &HashMap<&str, &str>) -> Env {
        let var = |k: &str| vars.get(k).map(|v| (*v).to_string());
        Env {
            home: var("HOME"),
            config_home: var("XDG_CONFIG_HOME"),
            state_home: var("XDG_STATE_HOME"),
            cache_home: var("XDG_CACHE_HOME"),
            runtime_dir: var("XDG_RUNTIME_DIR"),
            data_home: var("XDG_DATA_HOME"),
            socket: var(SOCKET_ENV),
        }
    }

    /// `$HOME`, or `/tmp` when the environment has none.
    #[must_use]
    pub fn home_dir(&self) -> PathBuf {
        home_of(self)
    }

    /// `$XDG_DATA_HOME`, defaulting to `~/.local/share`.
    #[must_use]
    pub fn data_home(&self) -> PathBuf {
        root(self.data_home.as_ref(), &home_of(self), ".local/share")
    }

    /// `$XDG_CONFIG_HOME`, defaulting to `~/.config`.
    #[must_use]
    pub fn config_home(&self) -> PathBuf {
        root(self.config_home.as_ref(), &home_of(self), ".config")
    }

    /// `$XDG_STATE_HOME`, defaulting to `~/.local/state`.
    #[must_use]
    pub fn state_home(&self) -> PathBuf {
        root(self.state_home.as_ref(), &home_of(self), ".local/state")
    }
}

/// Resolved file locations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config_file: PathBuf,
    pub state_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub runtime_dir: PathBuf,
    /// Where the macOS daemon listens; see [`Paths::socket_path`].
    pub socket: PathBuf,
}

fn home_of(env: &Env) -> PathBuf {
    env.home
        .as_deref()
        .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from)
}

fn root(explicit: Option<&String>, home: &Path, fallback: &str) -> PathBuf {
    match explicit {
        Some(value) => PathBuf::from(value),
        None => home.join(fallback),
    }
}

/// `$TOKEN_STATION_SOCKET`, or `daemon.sock` inside `runtime_dir`.
fn socket_in(env: &Env, runtime_dir: &Path) -> PathBuf {
    match env.socket.as_ref() {
        Some(value) => PathBuf::from(value),
        None => runtime_dir.join(SOCKET_FILE),
    }
}

/// The XDG layout, used on every platform but macOS.
#[must_use]
pub fn resolve_xdg(env: &Env) -> Paths {
    let home = home_of(env);
    let config = root(env.config_home.as_ref(), &home, ".config").join(APP_DIR);
    let state = root(env.state_home.as_ref(), &home, ".local/state").join(APP_DIR);
    let cache = root(env.cache_home.as_ref(), &home, ".cache").join(APP_DIR);
    // No runtime dir (cron, ssh without pam_systemd): keep the file with the state.
    let runtime = match env.runtime_dir.as_ref() {
        Some(value) => PathBuf::from(value).join(APP_DIR),
        None => state.join("run"),
    };
    Paths {
        config_file: config.join("config.toml"),
        state_dir: state,
        cache_dir: cache,
        socket: socket_in(env, &runtime),
        runtime_dir: runtime,
    }
}

/// The Apple layout: one `Application Support` directory for everything that
/// persists, `Caches` for what can be thrown away, and the socket beside the
/// state so the app, the CLI and the statusline hook all find it.
///
/// The XDG variables are deliberately ignored: a shell that exports them would
/// otherwise send the CLI to a different daemon than the one the menu bar app
/// launched.
#[must_use]
pub fn resolve_macos(env: &Env) -> Paths {
    let home = home_of(env);
    let support = home
        .join("Library")
        .join("Application Support")
        .join(MACOS_APP_DIR);
    let cache = home.join("Library").join("Caches").join(MACOS_APP_DIR);
    Paths {
        config_file: support.join("config.toml"),
        socket: socket_in(env, &support),
        state_dir: support.clone(),
        cache_dir: cache,
        runtime_dir: support,
    }
}

impl Paths {
    /// Resolve every location from `env`, in this platform's layout.
    #[must_use]
    pub fn resolve(env: &Env) -> Paths {
        if cfg!(target_os = "macos") {
            resolve_macos(env)
        } else {
            resolve_xdg(env)
        }
    }

    /// Paths for the current process environment.
    #[must_use]
    pub fn current() -> Paths {
        Paths::resolve(&Env::current())
    }

    /// Where the daemon listens, and where the clients connect.
    #[must_use]
    pub fn socket_path(&self) -> PathBuf {
        self.socket.clone()
    }

    /// The exclusive lock only the running daemon holds, beside the socket.
    ///
    /// Named after the socket rather than fixed, so two daemons pointed at two
    /// sockets in one directory — which is what `$TOKEN_STATION_SOCKET` is for —
    /// do not fight over a single lock and refuse each other.
    #[must_use]
    pub fn lock_file(&self) -> PathBuf {
        self.socket.with_extension(LOCK_EXTENSION)
    }

    #[must_use]
    pub fn history_db(&self) -> PathBuf {
        self.state_dir.join("history.sqlite")
    }

    #[must_use]
    pub fn alerts_file(&self) -> PathBuf {
        self.state_dir.join("alerts.json")
    }

    #[must_use]
    pub fn pricing_cache(&self) -> PathBuf {
        self.cache_dir.join("pricing.json")
    }

    /// Drop box used by `token-station statusline` when the daemon is not reachable.
    #[must_use]
    pub fn statusline_drop(&self) -> PathBuf {
        self.runtime_dir.join("claude-statusline.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Env {
        Env::from_map(&pairs.iter().copied().collect())
    }

    #[test]
    fn xdg_variables_win() {
        let p = resolve_xdg(&env(&[
            ("HOME", "/home/u"),
            ("XDG_CONFIG_HOME", "/cfg"),
            ("XDG_STATE_HOME", "/st"),
            ("XDG_CACHE_HOME", "/ca"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ]));
        assert_eq!(p.config_file, Path::new("/cfg/token-station/config.toml"));
        assert_eq!(
            p.history_db(),
            Path::new("/st/token-station/history.sqlite")
        );
        assert_eq!(p.alerts_file(), Path::new("/st/token-station/alerts.json"));
        assert_eq!(
            p.pricing_cache(),
            Path::new("/ca/token-station/pricing.json")
        );
        assert_eq!(
            p.statusline_drop(),
            Path::new("/run/user/1000/token-station/claude-statusline.json")
        );
    }

    #[test]
    fn falls_back_to_home_defaults() {
        let p = resolve_xdg(&env(&[("HOME", "/home/u")]));
        assert_eq!(
            p.config_file,
            Path::new("/home/u/.config/token-station/config.toml")
        );
        assert_eq!(
            p.history_db(),
            Path::new("/home/u/.local/state/token-station/history.sqlite")
        );
        assert_eq!(
            p.pricing_cache(),
            Path::new("/home/u/.cache/token-station/pricing.json")
        );
        // Without XDG_RUNTIME_DIR the drop box lives next to the state.
        assert_eq!(
            p.statusline_drop(),
            Path::new("/home/u/.local/state/token-station/run/claude-statusline.json")
        );
    }

    #[test]
    fn xdg_roots_have_spec_defaults() {
        let explicit = env(&[
            ("HOME", "/home/u"),
            ("XDG_DATA_HOME", "/da"),
            ("XDG_CONFIG_HOME", "/cfg"),
            ("XDG_STATE_HOME", "/st"),
        ]);
        assert_eq!(explicit.data_home(), Path::new("/da"));
        assert_eq!(explicit.config_home(), Path::new("/cfg"));
        assert_eq!(explicit.state_home(), Path::new("/st"));

        let bare = env(&[("HOME", "/home/u")]);
        assert_eq!(bare.data_home(), Path::new("/home/u/.local/share"));
        assert_eq!(bare.config_home(), Path::new("/home/u/.config"));
        assert_eq!(bare.state_home(), Path::new("/home/u/.local/state"));
        assert_eq!(bare.home_dir(), Path::new("/home/u"));
    }

    #[test]
    fn empty_home_still_resolves() {
        let p = Paths::resolve(&Env::default());
        assert!(p.config_file.starts_with("/tmp"));
        assert!(p.state_dir.ends_with(APP_DIR) || p.state_dir.ends_with(MACOS_APP_DIR));
    }

    #[test]
    fn the_socket_sits_in_the_runtime_dir_with_the_lock_beside_it() {
        let p = resolve_xdg(&env(&[
            ("HOME", "/home/u"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ]));
        assert_eq!(
            p.socket_path(),
            Path::new("/run/user/1000/token-station/daemon.sock")
        );
        assert_eq!(
            p.lock_file(),
            Path::new("/run/user/1000/token-station/daemon.lock")
        );
    }

    #[test]
    fn the_socket_override_moves_the_lock_with_it() {
        let p = resolve_xdg(&env(&[
            ("HOME", "/home/u"),
            ("TOKEN_STATION_SOCKET", "/tmp/ts/custom.sock"),
        ]));
        assert_eq!(p.socket_path(), Path::new("/tmp/ts/custom.sock"));
        // Named after the socket: two development daemons on two sockets in one
        // directory must not refuse each other over a shared lock.
        assert_eq!(p.lock_file(), Path::new("/tmp/ts/custom.lock"));

        let other = resolve_xdg(&env(&[
            ("HOME", "/home/u"),
            ("TOKEN_STATION_SOCKET", "/tmp/ts/a.sock"),
        ]));
        assert_eq!(other.lock_file(), Path::new("/tmp/ts/a.lock"));
        assert_ne!(other.lock_file(), p.lock_file());
    }

    #[test]
    fn macos_uses_the_apple_layout() {
        let p = resolve_macos(&env(&[("HOME", "/Users/u")]));
        let support = "/Users/u/Library/Application Support/dev.soldunov.TokenStation";
        assert_eq!(p.config_file, Path::new(support).join("config.toml"));
        assert_eq!(p.state_dir, Path::new(support));
        assert_eq!(p.runtime_dir, Path::new(support));
        assert_eq!(p.history_db(), Path::new(support).join("history.sqlite"));
        assert_eq!(p.alerts_file(), Path::new(support).join("alerts.json"));
        assert_eq!(p.socket_path(), Path::new(support).join("daemon.sock"));
        assert_eq!(p.lock_file(), Path::new(support).join("daemon.lock"));
        assert_eq!(
            p.statusline_drop(),
            Path::new(support).join("claude-statusline.json")
        );
        assert_eq!(
            p.pricing_cache(),
            Path::new("/Users/u/Library/Caches/dev.soldunov.TokenStation/pricing.json")
        );
    }

    #[test]
    fn macos_ignores_the_xdg_variables() {
        // A shell that exports these would otherwise send the CLI to a socket
        // the menu bar app never created.
        let p = resolve_macos(&env(&[
            ("HOME", "/Users/u"),
            ("XDG_CONFIG_HOME", "/cfg"),
            ("XDG_STATE_HOME", "/st"),
            ("XDG_CACHE_HOME", "/ca"),
            ("XDG_RUNTIME_DIR", "/run/user/501"),
        ]));
        assert_eq!(
            p.config_file,
            Path::new("/Users/u/Library/Application Support/dev.soldunov.TokenStation/config.toml")
        );
        assert!(p.socket_path().starts_with("/Users/u/Library"));

        // The socket override still works, because it is what the tests and the
        // development builds use to keep out of a real daemon's way.
        let overridden = resolve_macos(&env(&[
            ("HOME", "/Users/u"),
            ("TOKEN_STATION_SOCKET", "/tmp/ts-e2e.sock"),
        ]));
        assert_eq!(overridden.socket_path(), Path::new("/tmp/ts-e2e.sock"));
    }

    #[test]
    fn an_oversized_socket_path_is_refused() {
        let long = PathBuf::from(format!("/tmp/{}/daemon.sock", "x".repeat(120)));
        let error = check_socket_path(&long, 103).expect_err("too long");
        assert_eq!(error.max, 103);
        assert!(error.to_string().contains("more than the 103"), "{error}");
        // The macOS limit is the tighter of the two, so it is what a path has to
        // clear for the same layout to work on both.
        assert!(check_socket_path(Path::new("/tmp/ts.sock"), 103).is_ok());
        assert!(check_socket_path(&PathBuf::from("a".repeat(103)), 103).is_ok());
        assert!(check_socket_path(&PathBuf::from("a".repeat(104)), 103).is_err());
    }
}
