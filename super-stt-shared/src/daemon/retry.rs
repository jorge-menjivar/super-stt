// SPDX-License-Identifier: GPL-3.0-only
//! One connection-retry policy shared by every client that reconnects to the
//! daemon (settings app, applet, and the widget-subscription event bridge).
//!
//! Exponential backoff, capped, with ±10% jitter so N clients reconnecting after
//! the same daemon restart don't stampede in lockstep. Replaces three divergent
//! policies: the applet's own copy of this, the subscription bridge's
//! jitter-less doubling, and the app's flat 5 s sleep.

use std::time::Duration;

/// Whole milliseconds of a `Duration` as `u64`, saturating on the (practically
/// impossible) overflow.
fn duration_millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Connection retry strategy: exponential backoff between `initial_delay` and
/// `max_delay`, advanced by [`RetryStrategy::should_retry`] and reset by
/// [`RetryStrategy::reset`] on a successful connection.
#[derive(Debug, Clone)]
pub struct RetryStrategy {
    /// Current retry attempt number (the backoff exponent).
    pub attempt: u32,
    /// Delay for the first attempt.
    pub initial_delay: Duration,
    /// Ceiling on the backoff delay.
    pub max_delay: Duration,
    /// When false, every delay is exactly `initial_delay` (no growth, no jitter).
    pub use_exponential_backoff: bool,
}

impl Default for RetryStrategy {
    fn default() -> Self {
        Self {
            attempt: 0,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(15),
            use_exponential_backoff: true,
        }
    }
}

impl RetryStrategy {
    /// A strategy tuned for the initial daemon connection: quick first retries
    /// (500 ms) growing to a 15 s cap.
    #[must_use]
    pub fn for_initial_connection() -> Self {
        Self {
            attempt: 0,
            initial_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(15),
            use_exponential_backoff: true,
        }
    }

    /// The delay before the next attempt: `initial_delay * 2^attempt`, capped at
    /// `max_delay`, plus ±10% jitter.
    #[must_use]
    pub fn next_delay(&self) -> Duration {
        if !self.use_exponential_backoff {
            return self.initial_delay;
        }

        let base_delay = duration_millis(self.initial_delay);
        let exponential_delay = base_delay.saturating_mul(2_u64.saturating_pow(self.attempt));
        let capped_delay = exponential_delay.min(duration_millis(self.max_delay));

        // ±10% jitter to prevent a thundering herd. Skip entirely when the delay
        // is small enough that the jitter window rounds to zero (a `% 0` would
        // otherwise panic).
        let jitter_range = capped_delay / 10;
        if jitter_range == 0 {
            return Duration::from_millis(capped_delay);
        }
        let now_since_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let jitter = duration_millis(now_since_epoch) % (jitter_range * 2);
        Duration::from_millis(capped_delay + jitter - jitter_range)
    }

    /// Advance to the next attempt. Always returns `true` — the daemon
    /// connection is retried forever, just with a growing (capped) delay.
    pub fn should_retry(&mut self) -> bool {
        self.attempt += 1;
        true
    }

    /// Reset the attempt counter after a successful connection.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::RetryStrategy;
    use std::time::Duration;

    #[test]
    fn initial_connection_strategy_values() {
        let strategy = RetryStrategy::for_initial_connection();
        assert_eq!(strategy.attempt, 0);
        assert_eq!(strategy.initial_delay, Duration::from_millis(500));
        assert_eq!(strategy.max_delay, Duration::from_secs(15));
        assert!(strategy.use_exponential_backoff);
    }

    #[test]
    fn exponential_backoff_grows_and_caps() {
        let mut strategy = RetryStrategy::for_initial_connection();

        let first_delay = strategy.next_delay();
        assert!(first_delay >= Duration::from_millis(450)); // with jitter
        assert!(first_delay <= Duration::from_millis(550));

        strategy.should_retry();
        let second_delay = strategy.next_delay();
        assert!(second_delay > first_delay);

        for _ in 0..10 {
            strategy.should_retry();
        }
        let late_delay = strategy.next_delay();
        // Capped at 15 s + up to 10% jitter; 20 s is a generous bound.
        assert!(late_delay <= Duration::from_secs(20), "was {late_delay:?}");
    }

    #[test]
    fn retries_forever() {
        let mut strategy = RetryStrategy::for_initial_connection();
        for _ in 0..100 {
            assert!(strategy.should_retry());
        }
        assert!(strategy.should_retry());
    }

    #[test]
    fn reset_clears_attempts() {
        let mut strategy = RetryStrategy::for_initial_connection();
        for _ in 0..5 {
            strategy.should_retry();
        }
        assert_eq!(strategy.attempt, 5);
        strategy.reset();
        assert_eq!(strategy.attempt, 0);
    }

    #[test]
    fn small_initial_delay_does_not_panic_on_zero_jitter() {
        // A sub-10ms initial delay makes the jitter window round to 0; next_delay
        // must not divide by zero.
        let strategy = RetryStrategy {
            attempt: 0,
            initial_delay: Duration::from_millis(5),
            max_delay: Duration::from_secs(1),
            use_exponential_backoff: true,
        };
        assert_eq!(strategy.next_delay(), Duration::from_millis(5));
    }
}
