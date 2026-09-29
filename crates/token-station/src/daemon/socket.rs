//! The macOS front door: a Unix socket speaking JSON-RPC.
//!
//! No resume-from-sleep watcher lives here. The menu bar app is the one with a
//! wake notification worth having (`NSWorkspace.didWakeNotification`), and it
//! sends `Refresh` when it fires; a daemon that also watched for sleep would
//! only duplicate that.

use std::sync::Arc;

use anyhow::Context;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use ts_core::config::Config;

use crate::api::Api;
use crate::backend::Backend;
use crate::clock::system_clock;
use crate::daemon::run::{
    AlreadyRunning, DaemonOptions, build_daemon, load_config, resolve_paths, startup_work,
    wait_for_shutdown,
};
use crate::daemon::scheduler;
use crate::fixture::FixturePlayer;
use crate::ipc::hub::Hub;
use crate::ipc::server::{self, BindError};
use crate::notify::Notifier;
use crate::paths::Paths;
use crate::publish::{Publisher, SnapshotSink};

/// Run until SIGINT, SIGTERM or stdin EOF.
///
/// # Errors
///
/// Returns an error when another daemon holds the lock, when the socket cannot
/// be created, when the fixture file or the live providers cannot be served, or
/// when the signal handlers cannot be installed.
pub async fn run(options: DaemonOptions) -> anyhow::Result<()> {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let paths = resolve_paths(&options);

    // Taken before anything is opened, written or refreshed: a second daemon has
    // nothing useful to do, and half-finished work is how it would do harm.
    let listener = match server::bind(&paths.socket_path(), &paths.lock_file()) {
        Ok(listener) => listener,
        Err(BindError::AlreadyRunning) => return Err(AlreadyRunning.into()),
        Err(error) => return Err(anyhow::Error::new(error).context("cannot serve the socket")),
    };

    let config = load_config(&paths.config_file);
    // Built before the backend, because it is the backend's notifier as well as
    // the publisher's sink: an alert becomes an `Alert` notification for the app
    // to show, rather than a call to a notification daemon macOS does not have.
    let hub = Hub::new();
    let (api, loops) = match options.fixture.as_deref() {
        Some(path) => serve_fixture(path, config, &hub)?,
        None => serve_live(paths, config, &hub, &shutdown_rx).await?,
    };

    let serving = tokio::spawn(server::serve(listener, api, hub));
    wait_for_shutdown(options.exit_on_stdin_close).await?;
    tracing::info!("shutting down");
    let _ = shutdown_tx.send(true);
    for handle in loops {
        handle.abort();
    }
    // Aborting the accept loop drops the listener, which takes the socket file
    // with it, so the next daemon starts on a clean directory.
    serving.abort();
    let _ = serving.await;
    Ok(())
}

/// Point the hub at what the publisher already holds, and serve the backend.
///
/// The hub's state is seeded before it is attached, so the first thing a
/// subscriber is handed is the real snapshot rather than an empty placeholder.
fn wire(publisher: &Arc<Publisher>, backend: Arc<dyn Backend>, hub: &Arc<Hub>) -> Arc<Api> {
    let (revision, json) = publisher.published();
    hub.seed(revision, json);
    publisher.attach(Arc::clone(hub) as Arc<dyn SnapshotSink>);
    Arc::new(Api::new(Arc::clone(publisher), backend))
}

type Serving = (Arc<Api>, Vec<JoinHandle<()>>);

fn serve_fixture(
    path: &std::path::Path,
    config: Config,
    hub: &Arc<Hub>,
) -> anyhow::Result<Serving> {
    let player = FixturePlayer::load(path, config, system_clock())
        .with_context(|| format!("cannot load the fixture {}", path.display()))?;
    tracing::warn!(fixture = %path.display(), "serving a fixture; no real usage is read");
    let api = wire(
        &player.publisher(),
        Arc::clone(&player) as Arc<dyn Backend>,
        hub,
    );

    let ticker = Arc::clone(&player);
    let loops = vec![tokio::spawn(async move {
        loop {
            tokio::time::sleep(scheduler::TICK).await;
            ticker.republish().await;
        }
    })];
    Ok((api, loops))
}

async fn serve_live(
    paths: Paths,
    config: Config,
    hub: &Arc<Hub>,
    shutdown: &scheduler::Shutdown,
) -> anyhow::Result<Serving> {
    let daemon = build_daemon(paths, config, Arc::clone(hub) as Arc<dyn Notifier>).await?;
    let api = wire(
        &daemon.publisher,
        Arc::clone(&daemon) as Arc<dyn Backend>,
        hub,
    );

    let mut loops = scheduler::spawn_all(&daemon, shutdown);
    loops.push(tokio::spawn(startup_work(Arc::clone(&daemon))));
    Ok((api, loops))
}
