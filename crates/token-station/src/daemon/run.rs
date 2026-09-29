//! Wiring one daemon process together: bus name, loops, signals, shutdown.

use std::path::PathBuf;
use std::pin::pin;
use std::sync::Arc;

use anyhow::Context;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use ts_core::config::Config;
use ts_core::pricing::Pricing;
use zbus::export::futures_core::Stream;
use zbus::fdo::{DBusProxy, RequestNameFlags, RequestNameReply};

use crate::backend::Backend;
use crate::clock::system_clock;
use crate::daemon::scheduler;
use crate::daemon::state::Daemon;
use crate::dbus::{BUS_NAME, service};
use crate::fixture::FixturePlayer;
use crate::history::HistoryHandle;
use crate::notify::DesktopNotifier;
use crate::paths::Paths;
use crate::publish::Publisher;

/// Message shown when the well-known name is taken.
pub const ALREADY_RUNNING: &str = "another token-station daemon is already running";

/// A daemon already owns `dev.soldunov.TokenStation`.
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

/// Run until SIGINT or SIGTERM.
pub async fn run(options: DaemonOptions) -> anyhow::Result<()> {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let connection = zbus::Connection::session()
        .await
        .context("cannot connect to the session bus")?;
    // Asked before anything is opened, written or refreshed: a second daemon has
    // nothing useful to do, and half-finished work is how it would do harm.
    refuse_if_owned(&connection).await?;

    let paths = resolve_paths(&options);
    let config = load_config(&paths.config_file);
    let loops = match options.fixture.as_deref() {
        Some(path) => serve_fixture(&connection, path, config).await?,
        None => serve_live(&connection, paths, config, &shutdown_rx).await?,
    };
    tracing::info!("serving {BUS_NAME}");

    wait_for_signal().await?;
    tracing::info!("shutting down");
    let _ = shutdown_tx.send(true);
    for handle in loops {
        handle.abort();
    }
    Ok(())
}

fn resolve_paths(options: &DaemonOptions) -> Paths {
    let mut paths = Paths::current();
    if let Some(config) = options.config.clone() {
        paths.config_file = config;
    }
    paths
}

/// Publish the object, then take the name, then start the loops.
///
/// The object has to answer the very first call a `Type=dbus` unit or a bus
/// activation sends, so it is served before the name exists; nothing that reads
/// credentials or the network starts until the name is ours.
async fn claim(
    connection: &zbus::Connection,
    publisher: Arc<Publisher>,
    backend: Arc<dyn Backend>,
) -> anyhow::Result<()> {
    service::serve(connection, Arc::clone(&publisher), backend).await?;
    publisher.attach(connection.clone());
    claim_name(connection).await
}

async fn serve_fixture(
    connection: &zbus::Connection,
    path: &std::path::Path,
    config: Config,
) -> anyhow::Result<Vec<JoinHandle<()>>> {
    let player = FixturePlayer::load(path, config, system_clock())
        .with_context(|| format!("cannot load the fixture {}", path.display()))?;
    tracing::warn!(fixture = %path.display(), "serving a fixture; no real usage is read");
    claim(
        connection,
        player.publisher(),
        Arc::clone(&player) as Arc<dyn Backend>,
    )
    .await?;

    let ticker = Arc::clone(&player);
    Ok(vec![tokio::spawn(async move {
        loop {
            tokio::time::sleep(scheduler::TICK).await;
            ticker.republish().await;
        }
    })])
}

async fn serve_live(
    connection: &zbus::Connection,
    paths: Paths,
    config: Config,
    shutdown: &scheduler::Shutdown,
) -> anyhow::Result<Vec<JoinHandle<()>>> {
    let daemon = build_daemon(connection.clone(), paths, config).await?;
    claim(
        connection,
        Arc::clone(&daemon.publisher),
        Arc::clone(&daemon) as Arc<dyn Backend>,
    )
    .await?;

    let mut handles = scheduler::spawn_all(&daemon, shutdown);
    handles.push(tokio::spawn(startup_work(Arc::clone(&daemon))));
    handles.push(tokio::spawn(watch_for_resume(Arc::clone(&daemon))));
    Ok(handles)
}

/// Assemble the daemon, off the async threads and without opening anything.
///
/// The history database and the pricing cache are deliberately left untouched
/// here: this runs before the bus name is claimed (the object has to answer the
/// call that activated us), and a daemon that loses that race must not have
/// created, locked or read the files belonging to the one that won. The history
/// store opens on its first use and [`Daemon::merge_pricing_cache`] reads the
/// cache, both of them from [`startup_work`], which only runs once the name is
/// ours.
async fn build_daemon(
    connection: zbus::Connection,
    paths: Paths,
    config: Config,
) -> anyhow::Result<Arc<Daemon>> {
    let history = HistoryHandle::deferred(&paths.history_db());
    let notifier = Arc::new(DesktopNotifier::new(connection));
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
async fn startup_work(daemon: Arc<Daemon>) {
    daemon.merge_pricing_cache().await;
    daemon.refresh_all(false).await;
    daemon.prune_history().await;
    daemon.refresh_pricing().await;
}

/// Force a refresh when the machine wakes up, if logind is reachable.
async fn watch_for_resume(daemon: Arc<Daemon>) {
    let stream = match resume_signals().await {
        Ok(stream) => stream,
        Err(error) => {
            tracing::info!(%error, "logind unavailable; no resume-from-sleep refresh");
            return;
        }
    };
    let mut stream = pin!(stream);
    while let Some(message) = std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
        match message.body().deserialize::<bool>() {
            // `true` announces the suspend; `false` means we are awake again.
            Ok(false) => {
                tracing::info!("woke up, refreshing");
                daemon.refresh_all(true).await;
            }
            Ok(true) => {}
            Err(error) => tracing::debug!(%error, "unexpected PrepareForSleep payload"),
        }
    }
}

async fn resume_signals() -> zbus::Result<impl Stream<Item = zbus::Message>> {
    let connection = zbus::Connection::system().await?;
    let proxy = zbus::Proxy::new(
        &connection,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .await?;
    // Keep the connection alive for as long as the stream lives.
    let stream = proxy.receive_signal("PrepareForSleep").await?;
    Ok(KeepAlive {
        stream,
        _connection: connection,
    })
}

/// A signal stream that owns its connection.
struct KeepAlive<S> {
    stream: S,
    _connection: zbus::Connection,
}

impl<S: Stream + Unpin> Stream for KeepAlive<S> {
    type Item = S::Item;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<S::Item>> {
        std::pin::Pin::new(&mut self.get_mut().stream).poll_next(cx)
    }
}

/// Bail out early when the name already has an owner.
async fn refuse_if_owned(connection: &zbus::Connection) -> anyhow::Result<()> {
    let bus = DBusProxy::new(connection).await?;
    match bus.name_has_owner(BUS_NAME.try_into()?).await {
        Ok(true) => Err(AlreadyRunning.into()),
        // An unanswerable bus is the connection's problem, not a second daemon's.
        Ok(false) | Err(_) => Ok(()),
    }
}

/// Ask for the well-known name; refuse to run beside another daemon.
async fn claim_name(connection: &zbus::Connection) -> anyhow::Result<()> {
    let reply = connection
        .request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())
        .await;
    match reply {
        Ok(RequestNameReply::PrimaryOwner) => Ok(()),
        Ok(_) | Err(zbus::Error::NameTaken) => Err(AlreadyRunning.into()),
        Err(error) => Err(anyhow::Error::new(error).context("cannot request the bus name")),
    }
}

async fn wait_for_signal() -> anyhow::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).context("cannot listen for SIGTERM")?;
    let mut int = signal(SignalKind::interrupt()).context("cannot listen for SIGINT")?;
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
    Ok(())
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
        };
        let paths = resolve_paths(&options);
        assert_eq!(paths.config_file, PathBuf::from("/tmp/custom.toml"));
        assert_eq!(paths.state_dir, Paths::current().state_dir);
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
}
