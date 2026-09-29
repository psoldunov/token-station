//! The Linux front door: the well-known name on the session bus.

use std::pin::pin;
use std::sync::Arc;

use anyhow::Context;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use ts_core::config::Config;
use zbus::export::futures_core::Stream;
use zbus::fdo::{DBusProxy, RequestNameFlags, RequestNameReply};

use crate::backend::Backend;
use crate::clock::system_clock;
use crate::daemon::run::{
    AlreadyRunning, DaemonOptions, build_daemon, load_config, resolve_paths, startup_work,
    wait_for_shutdown,
};
use crate::daemon::scheduler;
use crate::daemon::state::Daemon;
use crate::dbus::sink::PropertiesSink;
use crate::dbus::{BUS_NAME, service};
use crate::fixture::FixturePlayer;
use crate::notify::DesktopNotifier;
use crate::paths::Paths;
use crate::publish::{Publisher, SnapshotSink};

/// Run until SIGINT or SIGTERM.
///
/// # Errors
///
/// Returns an error when the session bus is unreachable, when another daemon
/// already owns the well-known name, when the fixture file or the live
/// providers cannot be served, or when the signal handlers cannot be installed.
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

    wait_for_shutdown(options.exit_on_stdin_close).await?;
    tracing::info!("shutting down");
    let _ = shutdown_tx.send(true);
    for handle in loops {
        handle.abort();
    }
    Ok(())
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
    publisher.attach(Arc::new(PropertiesSink::new(connection.clone())) as Arc<dyn SnapshotSink>);
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
    let notifier = Arc::new(DesktopNotifier::new(connection.clone()));
    let daemon = build_daemon(paths, config, notifier).await?;
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
