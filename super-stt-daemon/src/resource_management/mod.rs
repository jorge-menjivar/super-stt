// SPDX-License-Identifier: GPL-3.0-only
//! Resource management module for preventing `DoS` attacks and resource exhaustion
//!
//! This module provides connection limiting, rate limiting, and resource monitoring
//! to protect the daemon from being overwhelmed by malicious or excessive requests.
mod connection;
mod manager;

pub use connection::ConnectionInfo;
pub use manager::{ResourceManager, ResourceStats};

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
            max_connections: 20, // More restrictive for production
            // The first-party settings app's own background polling
            // (5s keep-alive ping + 3s GPU refresh ≈ 32 req/min) plus the
            // ~14-request batch every reconnect/page-load fires already
            // approaches this ceiling; the limiter is a tight-loop backstop
            // for a buggy local client, not a security boundary (the daemon
            // binds a local Unix socket keyed per uid:pid), so it sits well
            // above legitimate first-party peak rather than at 1 req/s.
            max_requests_per_minute: 300,    // 5 requests per second
            max_requests_per_hour: 7200,     // headroom above ~32 req/min baseline
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
