//! A small exponential backoff shared by the refresh scheduler and the
//! persistent app-server respawn logic.

use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    initial: Duration,
    max: Duration,
    current: Duration,
    /// Set on failure; cleared on success. `None` means "not backing off".
    resume_at: Option<i64>,
}

impl Backoff {
    pub fn new(initial: Duration, max: Duration) -> Backoff {
        Backoff {
            initial,
            max,
            current: initial,
            resume_at: None,
        }
    }

    /// Record a failure at `now`; the next call is not allowed until the
    /// backoff elapses, and the delay doubles for next time (capped at `max`).
    pub fn fail(&mut self, now: i64) {
        let current = i64::try_from(self.current.as_secs()).unwrap_or(i64::MAX);
        self.resume_at = Some(now.saturating_add(current));
        self.current = (self.current * 2).min(self.max);
    }

    /// Clear the backoff after a success.
    pub fn succeed(&mut self) {
        self.current = self.initial;
        self.resume_at = None;
    }

    /// Whether a new attempt is allowed at `now`.
    pub fn ready(&self, now: i64) -> bool {
        self.resume_at.is_none_or(|t| now >= t)
    }

    /// Seconds until the next attempt is allowed (0 if already ready).
    pub fn remaining(&self, now: i64) -> Duration {
        match self.resume_at {
            // Guarded by `t > now`, so the difference is positive and exact.
            Some(t) if t > now => Duration::from_secs(t.saturating_sub(now).unsigned_abs()),
            _ => Duration::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backoff(initial_secs: u64, max_secs: u64) -> Backoff {
        Backoff::new(
            Duration::from_secs(initial_secs),
            Duration::from_secs(max_secs),
        )
    }

    #[test]
    fn doubles_on_repeated_failure_and_caps() {
        let mut b = backoff(60, 1800);
        b.fail(0);
        assert_eq!(b.remaining(0), Duration::from_secs(60));
        b.fail(60);
        assert_eq!(b.remaining(60), Duration::from_secs(120));
        for _ in 0..10 {
            b.fail(0);
        }
        assert!(b.remaining(0) <= Duration::from_secs(1800));
    }

    #[test]
    fn succeed_resets_to_initial() {
        let mut b = backoff(60, 1800);
        b.fail(0);
        b.fail(0);
        b.succeed();
        assert!(b.ready(0));
        b.fail(0);
        assert_eq!(b.remaining(0), Duration::from_secs(60));
    }

    #[test]
    fn ready_reflects_resume_at() {
        let mut b = backoff(10, 60);
        assert!(b.ready(0));
        b.fail(100);
        assert!(!b.ready(105));
        assert!(b.ready(110));
    }
}
