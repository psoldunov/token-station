//! Wiring one daemon process together: the parts both platforms share.
//!
//! What differs is only the front door and how the daemon learns it is the only
//! one: [`crate::daemon::bus`] takes a well-known name on the session bus,
//! [`crate::daemon::socket`] takes an exclusive lock beside a Unix socket.
//! Everything else — the config, the providers, the loops, the shutdown — is
//! the same code on both.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use ts_core::config::Config;
use ts_core::pricing::Pricing;

use crate::clock::system_clock;
use crate::daemon::state::Daemon;
use crate::history::HistoryHandle;
use crate::notify::Notifier;
use crate::paths::Paths;

/// Message shown when another daemon is already running.
pub const ALREADY_RUNNING: &str = "another token-station daemon is already running";

/// A daemon is already running for this user.
///
/// Reported to the caller as [`ALREADY_RUNNING_EXIT`], which the unit lists in
/// `RestartPreventExitStatus=`: restarting would only lose the same race again.
///
/// [`ALREADY_RUNNING_EXIT`]: crate::cli::ALREADY_RUNNING_EXIT
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{ALREADY_RUNNING}")]
pub struct AlreadyRunning;

/// How `token-station daemon` was invoked.
#[derive(Debug, Clone, Default)]
pub struct DaemonOptions {
    /// Override for `config.toml`.
    pub config: Option<PathBuf>,
    /// Serve a recorded snapshot instead of talking to the real CLIs.
    pub fixture: Option<PathBuf>,
    /// Shut down when stdin reaches EOF, as well as on SIGTERM.
    ///
    /// The macOS app spawns the daemon as a helper inside its own bundle and
    /// holds the other end of the pipe, so this is what stops a helper from
    /// outliving the app that started it — including when the app is killed and
    /// never gets to send a signal.
    pub exit_on_stdin_close: bool,
}

/// Load the config, logging (rather than failing on) an unusable file.
pub fn load_config(path: &std::path::Path) -> Config {
    match Config::load(path) {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(%error, "using default settings");
            Config::default()
        }
    }
}

/// Run until SIGINT, SIGTERM, or (with `--exit-on-stdin-close`) stdin EOF.
///
/// # Errors
///
/// Returns an error when the front door cannot be opened, when another daemon
/// is already running, when the fixture file or the live providers cannot be
/// served, or when the signal handlers cannot be installed.
pub async fn run(options: DaemonOptions) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        crate::daemon::socket::run(options).await
    }
    #[cfg(not(target_os = "macos"))]
    {
        crate::daemon::bus::run(options).await
    }
}

pub(crate) fn resolve_paths(options: &DaemonOptions) -> Paths {
    let mut paths = Paths::current();
    if let Some(config) = options.config.clone() {
        paths.config_file = config;
    }
    paths
}

/// Assemble the daemon, off the async threads and without opening anything.
///
/// The history database and the pricing cache are deliberately left untouched
/// here: this runs before the daemon is the only one (the D-Bus object has to
/// answer the call that activated us), and a daemon that loses that race must
/// not have created, locked or read the files belonging to the one that won.
/// The history store opens on its first use and [`Daemon::merge_pricing_cache`]
/// reads the cache, both of them from [`startup_work`], which only runs once
/// this process has won.
pub(crate) async fn build_daemon(
    paths: Paths,
    config: Config,
    notifier: Arc<dyn Notifier>,
) -> anyhow::Result<Arc<Daemon>> {
    let history = HistoryHandle::deferred(&paths.history_db());
    tokio::task::spawn_blocking(move || {
        Daemon::new(
            paths,
            config,
            Arc::new(std::sync::RwLock::new(Pricing::bundled())),
            history,
            notifier,
            system_clock(),
        )
    })
    .await
    .context("the daemon's startup work did not run")
}

/// The cached price table, the first refresh and the first pricing update.
pub(crate) async fn startup_work(daemon: Arc<Daemon>) {
    daemon.merge_pricing_cache().await;
    daemon.refresh_all(false).await;
    daemon.prune_history().await;
    daemon.refresh_pricing().await;
}

/// Wait for SIGINT, SIGTERM, or stdin EOF when `exit_on_stdin_close` is set.
///
/// # Errors
///
/// Returns an error when the signal handlers cannot be installed.
pub(crate) async fn wait_for_shutdown(exit_on_stdin_close: bool) -> anyhow::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).context("cannot listen for SIGTERM")?;
    let mut int = signal(SignalKind::interrupt()).context("cannot listen for SIGINT")?;
    let stdin_closed = std::pin::pin!(watch_stdin(exit_on_stdin_close));
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
        () = stdin_closed => tracing::info!("stdin closed"),
    }
    Ok(())
}

/// A future that resolves when stdin reaches its end, or never when the flag is
/// off.
///
/// The read runs on a thread of its own rather than on the runtime. A blocking
/// read on stdin cannot be cancelled — not `tokio::io::stdin`'s either, which
/// is a blocking read on a pool thread — so selecting over it directly would
/// leave SIGTERM waiting for a pipe nobody is ever going to write to or close.
/// Here the signal wins the race, the process exits, and the thread goes with
/// it.
///
/// With the flag off the future is pending for good, rather than a receiver
/// whose sender was dropped: that resolves straight away, and a `select!` arm
/// cannot tell "the pipe closed" from "there was never a pipe" — it would shut
/// the daemon down the moment it finished starting.
pub(crate) fn watch_stdin(enabled: bool) -> impl std::future::Future<Output = ()> + Send {
    let when_closed = enabled.then(|| {
        let (closed, when_closed) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            drain_to_eof(&mut std::io::stdin().lock());
            // The receiver is gone when something else shut us down first.
            let _ = closed.send(());
        });
        when_closed
    });
    async move {
        match when_closed {
            Some(when_closed) => {
                let _ = when_closed.await;
            }
            None => std::future::pending::<()>().await,
        }
    }
}

/// Read to the end, throwing away whatever arrives.
///
/// Nothing is meant to be written there; the pipe is a lifeline, and the read
/// ends exactly when the parent that holds the other end is gone.
fn drain_to_eof(source: &mut impl std::io::Read) {
    let mut buffer = [0u8; 256];
    loop {
        match source.read(&mut buffer) {
            Ok(0) => return,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => {
                tracing::warn!(%error, "cannot read stdin; treating it as closed");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Load a config written from `body`, in a directory of its own.
    fn load(body: &str) -> Config {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, body).expect("config written");
        load_config(&path)
    }

    #[expect(
        clippy::float_cmp,
        reason = "compares exact literals that never went through arithmetic"
    )]
    #[test]
    fn a_valid_config_is_used_and_an_unusable_one_falls_back() {
        assert_eq!(
            load("[alerts]\nwarning_percent = 65\n")
                .alerts
                .warning_percent,
            65.0
        );
        assert_eq!(load("[general\n"), Config::default());
        assert_eq!(load(""), Config::default());
    }

    #[test]
    fn the_config_override_replaces_only_that_path() {
        let options = DaemonOptions {
            config: Some(PathBuf::from("/tmp/custom.toml")),
            fixture: None,
            exit_on_stdin_close: false,
        };
        let paths = resolve_paths(&options);
        assert_eq!(paths.config_file, PathBuf::from("/tmp/custom.toml"));
        assert_eq!(paths.state_dir, Paths::current().state_dir);
        assert_eq!(paths.socket, Paths::current().socket);
    }

    #[test]
    fn the_already_running_error_survives_the_anyhow_wrapper() {
        let error: anyhow::Error = AlreadyRunning.into();
        assert_eq!(error.to_string(), ALREADY_RUNNING);
        assert_eq!(
            error.downcast_ref::<AlreadyRunning>(),
            Some(&AlreadyRunning)
        );
    }

    /// Without the flag, a daemon whose stdin is already closed — every daemon
    /// started from a shell with `&`, and every systemd unit — must keep
    /// running. A watcher that resolves rather than stays pending shuts it down
    /// the moment it finishes starting, which is a whole daemon that never runs.
    #[tokio::test]
    async fn without_the_flag_the_stdin_watcher_never_fires() {
        let watcher = std::pin::pin!(watch_stdin(false));
        let fired = tokio::time::timeout(std::time::Duration::from_millis(50), watcher).await;
        assert!(fired.is_err(), "reported a close nobody saw");
    }

    /// The same thing through the select the daemon actually waits on.
    #[tokio::test]
    async fn without_the_flag_shutdown_waits_for_a_signal() {
        let waiting = std::pin::pin!(wait_for_shutdown(false));
        let raced = tokio::time::timeout(std::time::Duration::from_millis(50), waiting).await;
        assert!(raced.is_err(), "shut down without being asked to");
    }

    /// The real EOF path, over a pipe this test owns both ends of.
    ///
    /// A socket pair rather than `std::io::pipe`, which needs a newer compiler
    /// than this workspace's `rust-version`.
    #[test]
    fn a_closed_pipe_is_read_to_its_end() {
        use std::io::Write;

        let (mut reader, mut writer) = std::os::unix::net::UnixStream::pair().expect("a pipe");
        let drained = std::thread::spawn(move || {
            drain_to_eof(&mut reader);
        });

        // Still open: whatever is written is thrown away and the read carries on.
        writer.write_all(b"ignored\n").expect("written");
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(!drained.is_finished(), "gave up while the pipe was open");

        drop(writer);
        drained.join().expect("the read ended with the pipe");
    }
}
