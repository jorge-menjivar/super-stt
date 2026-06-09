// SPDX-License-Identifier: GPL-3.0-only
use std::time::Duration;

/// Whole milliseconds of a `Duration` as `u64`, saturating on the
/// (practically impossible) overflow.
fn duration_millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Connection retry strategy configuration
#[derive(Debug, Clone)]
pub struct RetryStrategy {
    /// Current retry attempt number
    pub attempt: u32,
    /// Initial delay between retries
    pub initial_delay: Duration,
    /// Maximum delay between retries
    pub max_delay: Duration,
    /// Whether to use exponential backoff
    pub use_exponential_backoff: bool,
}

impl Default for RetryStrategy {
    fn default() -> Self {
        Self {
            attempt: 0,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(15), // Cap at 15 seconds between attempts
            use_exponential_backoff: true,
        }
    }
}

impl RetryStrategy {
    /// Create a retry strategy for initial daemon connection
    pub fn for_initial_connection() -> Self {
        Self {
            attempt: 0,
            initial_delay: Duration::from_millis(500), // Start with quick retries
            max_delay: Duration::from_secs(15),        // Cap at 15 seconds
            use_exponential_backoff: true,
        }
    }

    /// Calculate the next retry delay
    pub fn next_delay(&self) -> Duration {
        if !self.use_exponential_backoff {
            return self.initial_delay;
        }

        // Exponential backoff with jitter.
        let base_delay = duration_millis(self.initial_delay);
        let exponential_delay = base_delay.saturating_mul(2_u64.saturating_pow(self.attempt));
        let capped_delay = exponential_delay.min(duration_millis(self.max_delay));

        // Add ±10% jitter to prevent a thundering herd.
        let jitter_range = capped_delay / 10;
        let now_since_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let jitter = duration_millis(now_since_epoch) % (jitter_range * 2);
        let final_delay = capped_delay + jitter - jitter_range;

        Duration::from_millis(final_delay)
    }

    /// Increment the attempt counter and check if we should continue retrying
    /// Always returns true - retries forever with exponential backoff up to `max_delay`
    pub fn should_retry(&mut self) -> bool {
        self.attempt += 1;
        true // Always retry - never give up
    }

    /// Reset the retry strategy
    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}
