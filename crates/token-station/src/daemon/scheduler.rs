//! The daemon's timers: one limits loop per provider, one tokens loop, one tick.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use ts_core::ProviderId;

use crate::daemon::state::Daemon;

/// How often the daemon re-publishes so countdowns roll over on time.
pub const TICK: Duration = Duration::from_secs(60);
/// Never sleep longer than this, so config changes take effect promptly.
pub const MAX_SLEEP: Duration = Duration::from_secs(900);
/// How often the pricing cache is checked; it is good for a day, so this is only
/// about noticing that the day has passed on a daemon that never restarts.
pub const PRICING_CHECK: Duration = Duration::from_secs(60 * 60);

/// Broadcast shutdown to every loop.
pub type Shutdown = watch::Receiver<bool>;

/// Sleep, unless shutdown arrives first. Returns `false` when it is time to stop.
async fn sleep_or_stop(delay: Duration, shutdown: &mut Shutdown) -> bool {
    tokio::select! {
        () = tokio::time::sleep(delay.min(MAX_SLEEP)) => true,
        _ = shutdown.changed() => false,
    }
}

/// Plan-limit loop for one provider.
async fn limits_loop(daemon: Arc<Daemon>, id: ProviderId, mut shutdown: Shutdown) {
    loop {
        let interval = Duration::from_secs(daemon.config().general.limits_interval_secs);
        let delay = match daemon.provider(id) {
            Some(provider) => provider.next_limits_refresh(daemon.now(), interval),
            None => interval,
        };
        if !sleep_or_stop(delay, &mut shutdown).await {
            return;
        }
        daemon.refresh_limits(id, false).await;
    }
}

/// Local-log loop: token scan, config hot reload and the statusline drop box.
async fn tokens_loop(daemon: Arc<Daemon>, mut shutdown: Shutdown) {
    loop {
        let delay = Duration::from_secs(daemon.config().general.tokens_interval_secs);
        if !sleep_or_stop(delay, &mut shutdown).await {
            return;
        }
        daemon.reload_config_if_changed().await;
        daemon.ingest_statusline_file().await;
        daemon.refresh_tokens().await;
    }
}

/// Keeps countdowns honest and prunes history once a day.
async fn tick_loop(daemon: Arc<Daemon>, mut shutdown: Shutdown) {
    loop {
        if !sleep_or_stop(TICK, &mut shutdown).await {
            return;
        }
        daemon.republish().await;
        daemon.prune_history().await;
    }
}

/// Re-download the price table once a day, for a daemon that runs for weeks.
async fn pricing_loop(daemon: Arc<Daemon>, mut shutdown: Shutdown) {
    loop {
        if !sleep_or_stop(PRICING_CHECK, &mut shutdown).await {
            return;
        }
        daemon.refresh_pricing().await;
    }
}

/// Start every loop; the handles finish once `shutdown` flips.
pub fn spawn_all(daemon: &Arc<Daemon>, shutdown: &Shutdown) -> Vec<JoinHandle<()>> {
    let mut handles: Vec<JoinHandle<()>> = ProviderId::ALL
        .iter()
        .map(|id| tokio::spawn(limits_loop(Arc::clone(daemon), *id, shutdown.clone())))
        .collect();
    handles.push(tokio::spawn(tokens_loop(
        Arc::clone(daemon),
        shutdown.clone(),
    )));
    handles.push(tokio::spawn(tick_loop(
        Arc::clone(daemon),
        shutdown.clone(),
    )));
    handles.push(tokio::spawn(pricing_loop(
        Arc::clone(daemon),
        shutdown.clone(),
    )));
    handles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn sleeping_is_cut_short_by_shutdown() {
        let (tx, mut rx) = watch::channel(false);
        let waiter =
            tokio::spawn(async move { sleep_or_stop(Duration::from_secs(600), &mut rx).await });
        tokio::time::sleep(Duration::from_secs(1)).await;
        tx.send(true).unwrap();
        assert!(!waiter.await.unwrap());
    }

    #[tokio::test(start_paused = true)]
    async fn a_full_sleep_continues_the_loop() {
        let (_tx, mut rx) = watch::channel(false);
        assert!(sleep_or_stop(Duration::from_secs(5), &mut rx).await);
    }

    #[tokio::test(start_paused = true)]
    async fn sleeps_are_capped() {
        let (_tx, mut rx) = watch::channel(false);
        let started = tokio::time::Instant::now();
        sleep_or_stop(Duration::from_secs(86_400), &mut rx).await;
        assert_eq!(started.elapsed(), MAX_SLEEP);
    }
}
