// SPDX-License-Identifier: GPL-3.0-only
//! What a feature's daemon keeps to unload its model when idle and load it
//! again on demand. The setting is [`crate::config::IdleUnloadConfig`]; each
//! feature's own `model_management::idle` decides what "in use" means for it.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How often a daemon looks at whether its model has gone idle.
pub const CHECK_EVERY: Duration = Duration::from_secs(5);

/// One model slot's last use, and the lock its on-demand load holds.
#[derive(Debug)]
pub struct IdleTracker {
    last_used: Mutex<Instant>,
    /// Held across an on-demand load, so requests arriving together wait for
    /// one load. Without it each would start its own, and the load gate would
    /// make all but the newest give up.
    pub loading: tokio::sync::Mutex<()>,
    /// Held across an idle unload, from taking the model out of its slot to
    /// announcing it gone. Every load waits for it, so a model is never
    /// loaded while the one before it still holds its memory, and the
    /// announcement never follows the next model's.
    pub unloading: tokio::sync::Mutex<()>,
}

impl Default for IdleTracker {
    fn default() -> Self {
        Self {
            last_used: Mutex::new(Instant::now()),
            loading: tokio::sync::Mutex::new(()),
            unloading: tokio::sync::Mutex::new(()),
        }
    }
}

impl IdleTracker {
    fn last(&self) -> std::sync::MutexGuard<'_, Instant> {
        self.last_used
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Record a use of the model, now.
    pub fn touch(&self) {
        *self.last() = Instant::now();
    }

    /// How long since the model was last used.
    #[must_use]
    pub fn idle_for(&self) -> Duration {
        self.last().elapsed()
    }

    /// Make the model look unused for `by`.
    #[cfg(test)]
    pub(crate) fn rewind(&self, by: Duration) {
        let mut last = self.last();
        *last = last.checked_sub(by).expect("the clock predates the rewind");
    }
}
