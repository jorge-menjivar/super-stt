// SPDX-License-Identifier: GPL-3.0-only
use super::{ResourceError, ResourceLimits};
use chrono::{DateTime, Duration, Utc};
use std::net::SocketAddr;

/// Tracks request history for rate limiting
#[derive(Debug, Clone)]
pub(crate) struct RequestHistory {
    /// Timestamps of recent requests
    timestamps: Vec<DateTime<Utc>>,
    /// Last cleanup time
    last_cleanup: DateTime<Utc>,
}

impl RequestHistory {
    pub(crate) fn new() -> Self {
        Self {
            timestamps: Vec::new(),
            last_cleanup: Utc::now(),
        }
    }

    /// Add a new request timestamp and clean up old entries
    pub(crate) fn add_request(&mut self, now: DateTime<Utc>, window_seconds: u64) {
        self.timestamps.push(now);

        // Clean up old entries if needed (every 10 requests or 5 minutes)
        if self.timestamps.len().is_multiple_of(10)
            || now.signed_duration_since(self.last_cleanup) > Duration::minutes(5)
        {
            self.cleanup_old_entries(now, window_seconds * 60); // Keep 60 windows of history
            self.last_cleanup = now;
        }
    }

    /// Remove timestamps older than the specified window
    fn cleanup_old_entries(&mut self, now: DateTime<Utc>, max_age_seconds: u64) {
        let secs = i64::try_from(max_age_seconds).unwrap_or(i64::MAX);
        let cutoff = now - Duration::seconds(secs);
        self.timestamps.retain(|&timestamp| timestamp > cutoff);
    }

    /// Count requests within the specified window
    pub(crate) fn count_requests_in_window(&self, now: DateTime<Utc>, window_seconds: u64) -> u32 {
        let secs = i64::try_from(window_seconds).unwrap_or(i64::MAX);
        let window_start = now - Duration::seconds(secs);
        let count = self
            .timestamps
            .iter()
            .filter(|&&timestamp| timestamp > window_start)
            .count();
        u32::try_from(count).unwrap_or(u32::MAX)
    }
}

/// Connection information for resource tracking
#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    /// When the connection was established
    pub connected_at: DateTime<Utc>,
    /// Last activity timestamp
    pub last_activity: DateTime<Utc>,
    /// Request history for rate limiting
    pub(crate) request_history: RequestHistory,
    /// Connection identifier (client ID or generated)
    pub client_id: String,
    /// Optional client address for logging
    pub client_addr: Option<SocketAddr>,
}

impl ConnectionInfo {
    /// Create a new connection info
    #[must_use]
    pub fn new(client_id: String, client_addr: Option<SocketAddr>) -> Self {
        let now = Utc::now();
        Self {
            connected_at: now,
            last_activity: now,
            request_history: RequestHistory::new(),
            client_id,
            client_addr,
        }
    }

    /// Update the last activity timestamp
    pub fn update_activity(&mut self) {
        self.last_activity = Utc::now();
    }

    /// Check if connection has timed out
    #[must_use]
    pub fn is_timed_out(&self, timeout_seconds: u64) -> bool {
        let secs = i64::try_from(timeout_seconds).unwrap_or(i64::MAX);
        let timeout_duration = Duration::seconds(secs);
        Utc::now().signed_duration_since(self.last_activity) > timeout_duration
    }

    /// Add a request and check rate limits
    ///
    /// # Errors
    /// Returns an error if the rate limit is exceeded.
    pub fn add_request_and_check_limits(
        &mut self,
        limits: &ResourceLimits,
    ) -> Result<(), ResourceError> {
        let now = Utc::now();

        // Add the request to history
        self.request_history
            .add_request(now, limits.rate_limit_window_seconds);
        self.last_activity = now;

        // Check rate limits
        let requests_per_minute = self.request_history.count_requests_in_window(now, 60);
        if requests_per_minute > limits.max_requests_per_minute {
            return Err(ResourceError::RateLimitExceeded {
                requests: requests_per_minute,
                window: 60,
                limit: limits.max_requests_per_minute,
            });
        }

        let requests_per_hour = self.request_history.count_requests_in_window(now, 3600);
        if requests_per_hour > limits.max_requests_per_hour {
            return Err(ResourceError::RateLimitExceeded {
                requests: requests_per_hour,
                window: 3600,
                limit: limits.max_requests_per_hour,
            });
        }

        Ok(())
    }
}
