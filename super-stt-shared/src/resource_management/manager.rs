// SPDX-License-Identifier: GPL-3.0-only
use super::connection::ConnectionInfo;
use super::{ResourceError, ResourceLimits};
use chrono::Utc;
use log::{debug, warn};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;

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

    /// Register a connection for `client_id`, enforcing the connection cap.
    ///
    /// **Idempotent per `client_id`.** A client (keyed by `uid:pid`) may
    /// open several connections over its lifetime — a long-lived
    /// `/v1/events` stream alongside short `/v1/ping` calls, or simply a
    /// fresh connection per request. Re-registering an existing client is a
    /// no-op: its [`ConnectionInfo`] — and crucially its rolling
    /// `request_history` — is preserved, so the rate-limit window spans all
    /// of that client's connections rather than being reset by each new one.
    /// Only a genuinely new `client_id` allocates an entry and is subject to
    /// the connection cap (an existing client is already counted).
    ///
    /// # Errors
    /// Returns an error if registering a *new* client would exceed
    /// `max_connections`.
    pub async fn register_connection(
        &self,
        client_id: String,
        client_addr: Option<SocketAddr>,
    ) -> Result<(), ResourceError> {
        let mut connections = self.connections.write().await;

        // Existing client → keep its accumulated state untouched.
        if connections.contains_key(&client_id) {
            return Ok(());
        }

        // New client → enforce the cap before allocating its entry.
        if connections.len() >= self.limits.max_connections {
            return Err(ResourceError::ConnectionLimitExceeded {
                current: connections.len(),
                max: self.limits.max_connections,
            });
        }

        let conn_info = ConnectionInfo::new(client_id.clone(), client_addr);
        connections.insert(client_id, conn_info);

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

    /// Re-registering an existing client (e.g. it opened a second
    /// connection, or reconnected for the next request) must NOT reset its
    /// rolling request window. Regression guard: when `register_connection`
    /// overwrote the entry, a client could defeat the rate limit entirely by
    /// reconnecting between requests, and a sibling connection would wipe the
    /// accumulated history of an open one.
    #[tokio::test]
    async fn reregistering_a_client_preserves_its_rate_window() {
        let limits = ResourceLimits {
            max_requests_per_minute: 3,
            max_requests_per_hour: 10,
            ..Default::default()
        };
        let manager = ResourceManager::with_limits(limits);

        manager
            .register_connection("client1".to_string(), None)
            .await
            .unwrap();

        // Consume the whole per-minute budget on the first connection.
        for _ in 0..3 {
            assert!(manager.record_request("client1").await.is_ok());
        }

        // A second connection for the same client_id (re-registration) must
        // be a no-op that keeps the existing window — not a fresh start.
        manager
            .register_connection("client1".to_string(), None)
            .await
            .unwrap();

        // So the next request is still over quota. Before the fix this
        // reset the count and erroneously returned Ok.
        assert!(
            manager.record_request("client1").await.is_err(),
            "re-registration must not hand the client a fresh rate-limit budget"
        );
    }

    /// Re-registration is also exempt from the connection cap: an existing
    /// client that reconnects while the table is full is still allowed,
    /// since it's already counted. (A genuinely new client at the cap is
    /// rejected — covered by `test_connection_limiting`.)
    #[tokio::test]
    async fn reregistering_at_capacity_is_allowed() {
        let limits = ResourceLimits {
            max_connections: 1,
            ..Default::default()
        };
        let manager = ResourceManager::with_limits(limits);

        manager
            .register_connection("client1".to_string(), None)
            .await
            .unwrap();
        // Table is full, but re-registering the SAME client must succeed.
        assert!(
            manager
                .register_connection("client1".to_string(), None)
                .await
                .is_ok(),
            "an already-registered client must not be rejected by the cap"
        );
        // A different client at capacity is still rejected.
        assert!(
            manager
                .register_connection("client2".to_string(), None)
                .await
                .is_err()
        );
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
