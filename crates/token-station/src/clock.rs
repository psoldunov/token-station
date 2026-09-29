//! The one place that reads the wall clock, so tests can move time.

use std::sync::Arc;

/// Returns the current time in Unix seconds.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// The system clock.
#[must_use]
pub fn system_clock() -> Clock {
    Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
    })
}

/// A clock frozen at `now`.
#[must_use]
pub fn fixed_clock(now: i64) -> Clock {
    Arc::new(move || now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_clock_is_after_2025() {
        assert!(system_clock()() > 1_735_689_600);
    }

    #[test]
    fn fixed_clock_does_not_move() {
        let clock = fixed_clock(42);
        assert_eq!(clock(), 42);
        assert_eq!(clock(), 42);
    }
}
