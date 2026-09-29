//! Binding the socket, and who is allowed to talk to it.
//!
//! Three things guard the socket, because one is not enough on its own: the
//! directory and the socket are the user's alone (0700 and 0600), the daemon
//! checks every peer's user id, and an exclusive lock on `daemon.lock` means
//! only one daemon per user ever holds it.

use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::net::{UnixListener, UnixStream};

use crate::api::Api;
use crate::ipc::conn;
use crate::ipc::hub::Hub;
use crate::paths::{MAX_SOCKET_PATH_LEN, SocketPathTooLong, check_socket_path};

/// Why the daemon could not start listening.
#[derive(Debug, thiserror::Error)]
pub enum BindError {
    /// Another daemon holds the lock.
    #[error("{}", crate::daemon::run::ALREADY_RUNNING)]
    AlreadyRunning,
    #[error(transparent)]
    PathTooLong(#[from] SocketPathTooLong),
    #[error("cannot {what} {path}: {source}")]
    Io {
        what: &'static str,
        path: String,
        source: io::Error,
    },
}

fn io_error(what: &'static str, path: &Path) -> impl FnOnce(io::Error) -> BindError {
    let path = path.display().to_string();
    move |source| BindError::Io { what, path, source }
}

/// A bound socket, with the lock that says it is ours.
///
/// Dropping it takes the socket file away, so the next daemon finds a clean
/// directory rather than a stale path nothing answers on.
#[expect(
    clippy::struct_field_names,
    reason = "the socket and the lock are its other two fields; `socket` alone would read as the path"
)]
pub struct Listener {
    listener: UnixListener,
    socket: PathBuf,
    /// Held open for as long as the daemon runs: closing it drops the `flock`.
    _lock: std::fs::File,
}

impl Listener {
    /// Where it is listening.
    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.socket) {
            if error.kind() != io::ErrorKind::NotFound {
                tracing::warn!(%error, socket = %self.socket.display(), "cannot remove the socket");
            }
        }
    }
}

/// Take the lock, then bind `socket`.
///
/// The order matters: the stale socket of a daemon that crashed is only removed
/// once the lock is held, so a second daemon losing the race cannot delete the
/// socket of the one that won.
///
/// # Errors
///
/// Returns [`BindError::AlreadyRunning`] when another daemon holds the lock,
/// [`BindError::PathTooLong`] when `socket` does not fit in a `sockaddr_un`,
/// and [`BindError::Io`] for a directory, lock or socket that cannot be made.
pub fn bind(socket: &Path, lock: &Path) -> Result<Listener, BindError> {
    check_socket_path(socket, MAX_SOCKET_PATH_LEN)?;
    if let Some(parent) = socket.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .or_else(|error| match error.kind() {
                io::ErrorKind::AlreadyExists => Ok(()),
                _ => Err(io_error("create", parent)(error)),
            })?;
        tighten(parent)?;
    }

    let lock_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock)
        .map_err(io_error("open", lock))?;
    take_lock(&lock_file).map_err(|error| match error.kind() {
        io::ErrorKind::WouldBlock => BindError::AlreadyRunning,
        _ => io_error("lock", lock)(error),
    })?;

    // The lock is ours, so whatever is at `socket` belongs to a daemon that is
    // gone.
    match std::fs::remove_file(socket) {
        Ok(()) => tracing::info!(socket = %socket.display(), "removed a stale socket"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error("remove", socket)(error)),
    }

    let listener = UnixListener::bind(socket).map_err(io_error("bind", socket))?;
    if let Err(error) = std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600)) {
        // Bound but not private. Leaving it there would be a socket this daemon
        // never serves and another user might reach, so it goes.
        drop(listener);
        let _ = std::fs::remove_file(socket);
        return Err(io_error("chmod", socket)(error));
    }
    tracing::info!(socket = %socket.display(), "listening");
    Ok(Listener {
        listener,
        socket: socket.to_path_buf(),
        _lock: lock_file,
    })
}

/// Make sure the directory we are about to bind in is the user's alone.
///
/// `DirBuilder::mode` only applies to a directory this call creates. The
/// directory is shared with the config and the state on macOS, so something
/// else may well have made it first — and made it 0755. Only a directory this
/// user owns is touched: chmod-ing somebody else's would fail anyway, and
/// refusing on the mode alone would stop a daemon whose files are already
/// perfectly readable to it.
fn tighten(dir: &Path) -> Result<(), BindError> {
    let metadata = std::fs::metadata(dir).map_err(io_error("stat", dir))?;
    // SAFETY: `getuid` only reads process state; it cannot fail.
    let own_uid = unsafe { libc::getuid() };
    if metadata.uid() != own_uid {
        tracing::warn!(
            dir = %dir.display(),
            uid = metadata.uid(),
            "the socket directory belongs to another user; leaving its mode alone"
        );
        return Ok(());
    }
    if metadata.permissions().mode() & 0o777 == 0o700 {
        return Ok(());
    }
    tracing::info!(dir = %dir.display(), "tightening the socket directory to 0700");
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(io_error("chmod", dir))
}

/// An exclusive, non-blocking `flock` for the daemon's whole life.
fn take_lock(file: &std::fs::File) -> io::Result<()> {
    // SAFETY: `file` is open for the duration of the call, so the descriptor is
    // valid; `flock` only reads it.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(());
    }
    Err(io::Error::last_os_error())
}

/// The user id on the other end of `stream`.
///
/// `SO_PEERCRED` on Linux, `LOCAL_PEERCRED` on macOS; tokio picks.
fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    Ok(stream.peer_cred()?.uid())
}

/// Accept connections until the task is dropped.
///
/// Every accepted peer is checked against this process's own user id first: the
/// socket's mode already keeps other users out on every filesystem that honours
/// it, and this keeps them out on the ones that do not.
pub async fn serve(listener: Listener, api: Arc<Api>, hub: Arc<Hub>) {
    // SAFETY: `getuid` only reads process state; it cannot fail and touches no
    // memory of ours.
    let own_uid = unsafe { libc::getuid() };
    loop {
        let stream = match listener.listener.accept().await {
            Ok((stream, _)) => stream,
            Err(error) => {
                tracing::warn!(%error, "cannot accept a connection");
                // A failing accept that is retried straight away is a busy loop.
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                continue;
            }
        };
        match peer_uid(&stream) {
            Ok(uid) if uid == own_uid => {
                tokio::spawn(conn::handle(stream, Arc::clone(&api), Arc::clone(&hub)));
            }
            Ok(uid) => tracing::warn!(uid, "refusing a connection from another user"),
            Err(error) => tracing::warn!(%error, "cannot read the peer's user id; refusing"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(dir: &Path) -> (PathBuf, PathBuf) {
        (dir.join("daemon.sock"), dir.join("daemon.lock"))
    }

    #[tokio::test]
    async fn binding_creates_a_private_directory_and_socket() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (socket, lock) = paths(&dir.path().join("nested"));
        let listener = bind(&socket, &lock).expect("bound");

        let dir_mode = std::fs::metadata(socket.parent().expect("parent"))
            .expect("stat")
            .permissions()
            .mode();
        assert_eq!(dir_mode & 0o777, 0o700);
        let socket_mode = std::fs::metadata(&socket)
            .expect("stat")
            .permissions()
            .mode();
        assert_eq!(socket_mode & 0o777, 0o600);
        assert_eq!(listener.socket_path(), socket);
    }

    #[tokio::test]
    async fn an_existing_directory_is_tightened_to_0700() {
        let dir = tempfile::tempdir().expect("temp dir");
        let shared = dir.path().join("shared");
        // What another component leaves behind: the config directory, made
        // before the daemon ever ran, and world-readable.
        std::fs::create_dir(&shared).expect("created");
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        let _listener =
            bind(&shared.join("daemon.sock"), &shared.join("daemon.lock")).expect("bound");
        let mode = std::fs::metadata(&shared)
            .expect("stat")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[tokio::test]
    async fn a_second_daemon_on_the_same_directory_is_refused() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (socket, lock) = paths(dir.path());
        let _first = bind(&socket, &lock).expect("bound");
        let Err(error) = bind(&socket, &lock) else {
            panic!("a second daemon was allowed to bind");
        };
        assert!(matches!(error, BindError::AlreadyRunning), "{error}");
        assert_eq!(error.to_string(), crate::daemon::run::ALREADY_RUNNING);
    }

    #[tokio::test]
    async fn a_stale_socket_is_removed_and_the_file_goes_with_the_daemon() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (socket, lock) = paths(dir.path());
        // What a daemon that was killed leaves behind.
        std::fs::write(&socket, b"stale").expect("stale socket");

        let listener = bind(&socket, &lock).expect("bound");
        assert!(socket.exists());
        drop(listener);
        assert!(!socket.exists(), "a clean shutdown takes the socket away");

        // And the lock is free again, so the next daemon starts.
        drop(rebind(&socket, &lock));
    }

    /// Bind again, allowing for a lock another thread is briefly holding open.
    ///
    /// Not a product race: an `flock` is released when the last descriptor on it
    /// closes, and this one's closed with the listener above. But a `fork`
    /// anywhere else in this *test binary* duplicates every open descriptor for
    /// the microseconds until the child `exec`s and `CLOEXEC` takes them away —
    /// and the tests that spawn `sh` run beside this one. A daemon has no such
    /// neighbours, so the retry belongs here rather than in `bind`.
    fn rebind(socket: &Path, lock: &Path) -> Listener {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match bind(socket, lock) {
                Ok(listener) => return listener,
                Err(BindError::AlreadyRunning) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(error) => panic!("the lock was never released: {error}"),
            }
        }
    }

    #[test]
    fn an_oversized_path_is_refused_before_anything_is_created() {
        let dir = tempfile::tempdir().expect("temp dir");
        let long = dir.path().join("x".repeat(MAX_SOCKET_PATH_LEN + 1));
        let Err(error) = bind(&long, &dir.path().join("daemon.lock")) else {
            panic!("an oversized path was accepted");
        };
        assert!(matches!(error, BindError::PathTooLong(_)), "{error}");
        assert!(!dir.path().join("daemon.lock").exists());
    }
}
