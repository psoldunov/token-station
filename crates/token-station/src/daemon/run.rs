//! Wiring one daemon process together: bus name, loops, signals, shutdown.

use std::path::PathBuf;
use std::pin::pin;
use std::sync::Arc;

use anyhow::{Context, anyhow};
use tokio::sync::watch;
use ts_core::config::Config;
use zbus::export::futures_core::Stream;
use zbus::fdo::{RequestNameFlags, RequestNameReply};

use crate::backend::Backend;
use crate::clock::system_clock;
use crate::daemon::scheduler;
use crate::daemon::state::Daemon;
use crate::dbus::{BUS_NAME, service};
use crate::fixture::FixturePlayer;
use crate::history::HistoryHandle;
use crate::notify::DesktopNotifier;
use crate::paths::Paths;
use crate::pricing_refresh;
use crate::publish::Publisher;

/// Message shown when the well-known name is taken.
pub const ALREADY_RUNNING: &str = "another token-station daemon is already running";

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
    let paths = resolve_paths(&options);
    let config = load_config(&paths.config_file);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let connection = zbus::Connection::session()
        .await
        .context("cannot connect to the session bus")?;

    let (publisher, backend, loops) = match options.fixture.as_deref() {
        Some(path) => start_fixture(path, config)?,
        None => start_live(paths, config, &shutdown_rx)?,
    };

    service::serve(&connection, Arc::clone(&publisher), backend).await?;
    publisher.attach(connection.clone());
    claim_name(&connection).await?;
    tracing::info!("serving {BUS_NAME}");

    wait_for_signal().await?;
    tracing::info!("shutting down");
    let _ = shutdown_tx.send(true);
    for handle in loops {
        handle.abort();
    }
    Ok(())
}

type Started = (
    Arc<Publisher>,
    Arc<dyn Backend>,
    Vec<tokio::task::JoinHandle<()>>,
);

fn resolve_paths(options: &DaemonOptions) -> Paths {
    let mut paths = Paths::current();
    if let Some(config) = options.config.clone() {
        paths.config_file = config;
    }
    paths
}

fn start_fixture(path: &std::path::Path, config: Config) -> anyhow::Result<Started> {
    let player = FixturePlayer::load(path, config, system_clock())
        .with_context(|| format!("cannot load the fixture {}", path.display()))?;
    tracing::warn!(fixture = %path.display(), "serving a fixture; no real usage is read");
    let publisher = player.publisher();
    let ticker = Arc::clone(&player);
    let handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(scheduler::TICK).await;
            ticker.republish().await;
        }
    });
    Ok((publisher, player, vec![handle]))
}

fn start_live(
    paths: Paths,
    config: Config,
    shutdown: &scheduler::Shutdown,
) -> anyhow::Result<Started> {
    let pricing = Arc::new(std::sync::RwLock::new(pricing_refresh::bundled_with_cache(
        &paths.pricing_cache(),
    )));
    let history = HistoryHandle::open_or_memory(&paths.history_db())
        .context("cannot open the history database")?;
    let daemon = Daemon::new(
        paths,
        config,
        pricing,
        history,
        Arc::new(DesktopNotifier),
        system_clock(),
    );

    let mut handles = scheduler::spawn_all(&daemon, shutdown);
    handles.push(tokio::spawn(startup_work(Arc::clone(&daemon))));
    handles.push(tokio::spawn(watch_for_resume(Arc::clone(&daemon))));
    Ok((Arc::clone(&daemon.publisher), daemon, handles))
}

/// The first refresh plus the daily pricing update.
async fn startup_work(daemon: Arc<Daemon>) {
    daemon.refresh_all(false).await;
    daemon.prune_history().await;
    let config = daemon.config();
    let cache = daemon.paths.pricing_cache();
    pricing_refresh::refresh_if_due(&config.pricing, &cache, &daemon.pricing(), daemon.now()).await;
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

/// Ask for the well-known name; refuse to run beside another daemon.
async fn claim_name(connection: &zbus::Connection) -> anyhow::Result<()> {
    let reply = connection
        .request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())
        .await;
    match reply {
        Ok(RequestNameReply::PrimaryOwner) => Ok(()),
        Ok(_) | Err(zbus::Error::NameTaken) => Err(anyhow!(ALREADY_RUNNING)),
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

    #[test]
    fn an_unreadable_config_falls_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[general\n").unwrap();
        assert_eq!(load_config(&path), Config::default());
    }

    #[test]
    fn a_valid_config_is_used() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[alerts]\nwarning_percent = 65\n").unwrap();
        assert_eq!(load_config(&path).alerts.warning_percent, 65.0);
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
}
