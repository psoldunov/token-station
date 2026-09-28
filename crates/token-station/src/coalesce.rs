//! One shared run for concurrent callers.
//!
//! `Refresh()` may arrive from several front ends at once; they all wait for the
//! same underlying refresh instead of queueing one each.

use std::future::Future;
use std::sync::Mutex;

use tokio::sync::watch;

/// Runs a closure at most once at a time; latecomers await the run in flight.
pub struct Coalescer {
    running: Mutex<bool>,
    generation: watch::Sender<u64>,
}

impl Default for Coalescer {
    fn default() -> Self {
        Coalescer {
            running: Mutex::new(false),
            generation: watch::channel(0).0,
        }
    }
}

impl Coalescer {
    /// Run `work`, or wait for the run already in progress to finish.
    pub async fn run<F, Fut>(&self, work: F)
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = ()>,
    {
        // Subscribe before claiming leadership so no completion can be missed.
        let mut updates = self.generation.subscribe();
        let start = *updates.borrow_and_update();

        let leader = {
            let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
            let free = !*running;
            *running = true;
            free
        };

        if leader {
            // A guard, not a straight line: a refresh that panics must still hand
            // leadership back, or `Refresh()` is wedged for the rest of the process.
            let _release = Release {
                running: &self.running,
                generation: &self.generation,
            };
            work().await;
            return;
        }

        while *updates.borrow_and_update() <= start {
            if updates.changed().await.is_err() {
                return;
            }
        }
    }
}

/// Clears the running flag and wakes the followers, whether the run returned or
/// unwound.
struct Release<'a> {
    running: &'a Mutex<bool>,
    generation: &'a watch::Sender<u64>,
}

impl Drop for Release<'_> {
    fn drop(&mut self) {
        *self.running.lock().unwrap_or_else(|e| e.into_inner()) = false;
        self.generation.send_modify(|value| *value += 1);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn concurrent_callers_share_one_run() {
        let coalescer = Arc::new(Coalescer::default());
        let runs = Arc::new(AtomicUsize::new(0));

        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let coalescer = Arc::clone(&coalescer);
                let runs = Arc::clone(&runs);
                tokio::spawn(async move {
                    coalescer
                        .run(|| async {
                            runs.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(60)).await;
                        })
                        .await;
                })
            })
            .collect();
        for task in tasks {
            task.await.unwrap();
        }
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn sequential_calls_each_run() {
        let coalescer = Coalescer::default();
        let runs = AtomicUsize::new(0);
        for _ in 0..3 {
            coalescer
                .run(|| async {
                    runs.fetch_add(1, Ordering::SeqCst);
                })
                .await;
        }
        assert_eq!(runs.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn followers_return_only_after_the_run_finished() {
        let coalescer = Arc::new(Coalescer::default());
        let done = Arc::new(AtomicUsize::new(0));

        let leader = {
            let coalescer = Arc::clone(&coalescer);
            let done = Arc::clone(&done);
            tokio::spawn(async move {
                coalescer
                    .run(|| async {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        done.store(1, Ordering::SeqCst);
                    })
                    .await;
            })
        };
        tokio::time::sleep(Duration::from_millis(10)).await;
        coalescer.run(|| async {}).await;
        assert_eq!(done.load(Ordering::SeqCst), 1);
        leader.await.unwrap();
    }

    #[tokio::test]
    async fn a_panicking_run_does_not_wedge_the_gate() {
        let coalescer = Arc::new(Coalescer::default());
        let panicked = {
            let coalescer = Arc::clone(&coalescer);
            tokio::spawn(async move {
                coalescer
                    .run(|| async { panic!("a provider blew up mid-refresh") })
                    .await;
            })
        };
        assert!(panicked.await.is_err(), "the panic reached the task");

        // The next caller still leads, instead of waiting for a run that is gone.
        let runs = AtomicUsize::new(0);
        coalescer
            .run(|| async {
                runs.fetch_add(1, Ordering::SeqCst);
            })
            .await;
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }
}
