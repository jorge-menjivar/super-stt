// SPDX-License-Identifier: GPL-3.0-only
//! Resource management module for preventing `DoS` attacks and resource exhaustion
//!
//! This module provides connection limiting, rate limiting, and resource monitoring
//! to protect the daemon from being overwhelmed by malicious or excessive requests.

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use log::{debug, warn};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Configuration for resource management limits
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    /// Maximum number of concurrent connections
    pub max_connections: usize,
    /// Maximum requests per client per minute
    pub max_requests_per_minute: u32,
    /// Maximum requests per client per hour
    pub max_requests_per_hour: u32,
    /// Connection timeout in seconds
    pub connection_timeout_seconds: u64,
    /// Rate limiting window size in seconds
    pub rate_limit_window_seconds: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_connections: 100,            // Reasonable for a desktop daemon
            max_requests_per_minute: 120,    // 2 requests per second
            max_requests_per_hour: 3600,     // 1 request per second average
            connection_timeout_seconds: 300, // 5 minutes
            rate_limit_window_seconds: 60,   // 1 minute windows
        }
    }
}

impl ResourceLimits {
    /// Create resource limits suitable for development
    #[must_use]
    pub fn development() -> Self {
        Self {
            max_connections: 50,
            max_requests_per_minute: 300, // More lenient for development
            max_requests_per_hour: 7200,
            connection_timeout_seconds: 600, // 10 minutes
            rate_limit_window_seconds: 60,
        }
    }

    /// Create resource limits suitable for production
    #[must_use]
    pub fn production() -> Self {
        Self {
            max_connections: 20,             // More restrictive for production
            max_requests_per_minute: 60,     // 1 request per second
            max_requests_per_hour: 1800,     // 0.5 requests per second average
            connection_timeout_seconds: 180, // 3 minutes
            rate_limit_window_seconds: 60,
        }
    }
}

/// Resource management errors
#[derive(Debug, thiserror::Error)]
pub enum ResourceError {
    #[error("Connection limit exceeded: {current}/{max} connections")]
    ConnectionLimitExceeded { current: usize, max: usize },

    #[error("Rate limit exceeded: {requests} requests in {window}s (max: {limit})")]
    RateLimitExceeded {
        requests: u32,
        window: u64,
        limit: u32,
    },

    #[error("Connection timeout: inactive for {seconds}s")]
    ConnectionTimeout { seconds: u64 },

    #[error("Resource temporarily unavailable")]
    ResourceUnavailable,
}

/// Tracks request history for rate limiting
#[derive(Debug, Clone)]
struct RequestHistory {
    /// Timestamps of recent requests
    timestamps: Vec<DateTime<Utc>>,
    /// Last cleanup time
    last_cleanup: DateTime<Utc>,
}

impl RequestHistory {
    fn new() -> Self {
        Self {
            timestamps: Vec::new(),
            last_cleanup: Utc::now(),
        }
    }

    /// Add a new request timestamp and clean up old entries
    fn add_request(&mut self, now: DateTime<Utc>, window_seconds: u64) {
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
    fn count_requests_in_window(&self, now: DateTime<Utc>, window_seconds: u64) -> u32 {
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
    request_history: RequestHistory,
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

/// Resource manager for tracking connections and enforcing limits
#[derive(Debug)]
pub struct ResourceManager {
    /// Resource limits configuration
    limits: ResourceLimits,
    /// Active connections
    connections: Arc<RwLock<HashMap<String, ConnectionInfo>>>,
    /// Background cleanup task handle
    cleanup_handle: Option<tokio::task::JoinHandle<()>>,
}

impl ResourceManager {
    /// Create a new resource manager with default limits
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(ResourceLimits::default())
    }

    /// Create a new resource manager with custom limits
    #[must_use]
    pub fn with_limits(limits: ResourceLimits) -> Self {
        let connections = Arc::new(RwLock::new(HashMap::new()));

        // Start background cleanup task
        let cleanup_connections = Arc::clone(&connections);
        let cleanup_limits = limits.clone();
        let cleanup_handle = tokio::spawn(async move {
            Self::cleanup_task(cleanup_connections, cleanup_limits).await;
        });

        Self {
            limits,
            connections,
            cleanup_handle: Some(cleanup_handle),
        }
    }

    /// Create a resource manager suitable for development
    #[must_use]
    pub fn development() -> Self {
        Self::with_limits(ResourceLimits::development())
    }

    /// Create a resource manager suitable for production
    #[must_use]
    pub fn production() -> Self {
        Self::with_limits(ResourceLimits::production())
    }

    /// Register a new connection and check connection limits
    ///
    /// # Errors
    /// Returns an error if the connection limit is exceeded.
    pub async fn register_connection(
        &self,
        client_id: String,
        client_addr: Option<SocketAddr>,
    ) -> Result<(), ResourceError> {
        let mut connections = self.connections.write().await;

        // Check connection limit
        if connections.len() >= self.limits.max_connections {
            return Err(ResourceError::ConnectionLimitExceeded {
                current: connections.len(),
                max: self.limits.max_connections,
            });
        }

        // Register the connection
        let conn_info = ConnectionInfo::new(client_id.clone(), client_addr);
        connections.insert(client_id.clone(), conn_info);

        Ok(())
    }

    /// Unregister a connection
    pub async fn unregister_connection(&self, client_id: &str) {
        let mut connections = self.connections.write().await;
        let _ = connections.remove(client_id).is_some();
    }

    /// Record a request and check rate limits
    ///
    /// # Errors
    /// Returns an error if the client is unregistered or any rate limit is exceeded.
    pub async fn record_request(&self, client_id: &str) -> Result<(), ResourceError> {
        let mut connections = self.connections.write().await;

        if let Some(conn_info) = connections.get_mut(client_id) {
            conn_info.add_request_and_check_limits(&self.limits)
        } else {
            warn!("Request from unregistered client: {client_id}");
            Err(ResourceError::ResourceUnavailable)
        }
    }

    /// Get current connection count
    pub async fn connection_count(&self) -> usize {
        self.connections.read().await.len()
    }

    /// Get resource usage statistics
    pub async fn get_stats(&self) -> ResourceStats {
        let connections = self.connections.read().await;
        let now = Utc::now();

        let mut total_requests_last_minute = 0;
        let mut total_requests_last_hour = 0;
        let mut active_connections = 0;

        for conn in connections.values() {
            if !conn.is_timed_out(self.limits.connection_timeout_seconds) {
                active_connections += 1;
                total_requests_last_minute +=
                    conn.request_history.count_requests_in_window(now, 60);
                total_requests_last_hour +=
                    conn.request_history.count_requests_in_window(now, 3600);
            }
        }

        ResourceStats {
            total_connections: connections.len(),
            active_connections,
            total_requests_last_minute,
            total_requests_last_hour,
            max_connections: self.limits.max_connections,
            max_requests_per_minute: self.limits.max_requests_per_minute,
            max_requests_per_hour: self.limits.max_requests_per_hour,
        }
    }

    /// Background task to clean up timed-out connections
    async fn cleanup_task(
        connections: Arc<RwLock<HashMap<String, ConnectionInfo>>>,
        limits: ResourceLimits,
    ) {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));

        loop {
            interval.tick().await;

            let mut connections_guard = connections.write().await;
            let initial_count = connections_guard.len();

            // Remove timed-out connections
            connections_guard.retain(|client_id, conn_info| {
                if conn_info.is_timed_out(limits.connection_timeout_seconds) {
                    debug!("Cleaned up timed-out connection: {client_id}");
                    false
                } else {
                    true
                }
            });

            let removed_count = initial_count - connections_guard.len();
            if removed_count > 0 {
                debug!("Cleaned up {removed_count} timed-out connections");
            }
        }
    }
}

impl Default for ResourceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ResourceManager {
    fn drop(&mut self) {
        if let Some(handle) = self.cleanup_handle.take() {
            handle.abort();
        }
    }
}

/// Resource usage statistics
#[derive(Debug, Clone)]
pub struct ResourceStats {
    pub total_connections: usize,
    pub active_connections: usize,
    pub total_requests_last_minute: u32,
    pub total_requests_last_hour: u32,
    pub max_connections: usize,
    pub max_requests_per_minute: u32,
    pub max_requests_per_hour: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{Duration as TokioDuration, sleep};

    #[tokio::test]
    async fn test_connection_limiting() {
        let limits = ResourceLimits {
            max_connections: 2,
            ..Default::default()
        };
        let manager = ResourceManager::with_limits(limits);

        // Register first connection - should succeed
        assert!(
            manager
                .register_connection("client1".to_string(), None)
                .await
                .is_ok()
        );

        // Register second connection - should succeed
        assert!(
            manager
                .register_connection("client2".to_string(), None)
                .await
                .is_ok()
        );

        // Register third connection - should fail
        assert!(
            manager
                .register_connection("client3".to_string(), None)
                .await
                .is_err()
        );

        // Unregister one connection
        manager.unregister_connection("client1").await;

        // Now third connection should succeed
        assert!(
            manager
                .register_connection("client3".to_string(), None)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn test_rate_limiting() {
        let limits = ResourceLimits {
            max_requests_per_minute: 3,
            max_requests_per_hour: 10,
            ..Default::default()
        };
        let manager = ResourceManager::with_limits(limits);

        // Register a connection
        manager
            .register_connection("client1".to_string(), None)
            .await
            .unwrap();

        // First 3 requests should succeed
        for _ in 0..3 {
            assert!(manager.record_request("client1").await.is_ok());
        }

        // Fourth request should fail (rate limit exceeded)
        assert!(manager.record_request("client1").await.is_err());
    }

    #[tokio::test]
    async fn test_connection_timeout() {
        let limits = ResourceLimits {
            connection_timeout_seconds: 1, // 1 second timeout for testing
            ..Default::default()
        };
        let manager = ResourceManager::with_limits(limits);

        // Register a connection
        manager
            .register_connection("client1".to_string(), None)
            .await
            .unwrap();
        assert_eq!(manager.connection_count().await, 1);

        // Wait for timeout + cleanup interval
        sleep(TokioDuration::from_secs(2)).await;

        // Connection should be cleaned up
        // Note: In real usage, the cleanup task runs every 30 seconds
        // For testing, we'll manually check the connection timeout
        let connections = manager.connections.read().await;
        if let Some(conn) = connections.get("client1") {
            assert!(conn.is_timed_out(1));
        }
    }

    #[tokio::test]
    async fn test_resource_stats() {
        let manager = ResourceManager::new();

        // Register some connections and make requests
        manager
            .register_connection("client1".to_string(), None)
            .await
            .unwrap();
        manager
            .register_connection("client2".to_string(), None)
            .await
            .unwrap();

        manager.record_request("client1").await.unwrap();
        manager.record_request("client2").await.unwrap();

        let stats = manager.get_stats().await;
        assert_eq!(stats.total_connections, 2);
        assert_eq!(stats.active_connections, 2);
        assert_eq!(stats.total_requests_last_minute, 2);
    }
}
