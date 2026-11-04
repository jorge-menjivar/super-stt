// SPDX-License-Identifier: GPL-3.0-only
//! How much the daemon serves at once, and how fast it lets one client ask.
//!
//! The limiter is a backstop against a local client stuck in a tight loop, not
//! a security boundary: the socket is the user's own, and a client is told
//! apart by its uid and pid. So the limits sit well above what the first-party
//! clients ask for, and one daemon serving both features has twice the
//! clients the speech-to-text daemon did — two apps, two CLIs, an applet
//! following both — so it has twice the room.

use crate::resource_management::{ResourceLimits, ResourceManager};

/// Concurrent connections, across every client.
pub const MAX_CONNECTIONS: usize = 40;

/// Requests one client may make in a rolling minute. The `count > limit`
/// check makes request `MAX_REQUESTS_PER_MINUTE + 1` the first refused.
pub const MAX_REQUESTS_PER_MINUTE: u32 = 600;

/// Requests one client may make in a rolling hour: the minute's ceiling,
/// sustained for 24 of its minutes.
pub const MAX_REQUESTS_PER_HOUR: u32 = 14_400;

/// The daemon's limits. The same in every build, so what a test daemon
/// refuses is what an installed one does; only how long an idle connection
/// is kept comes from the engine's preset for the build.
#[must_use]
pub fn resource_limits() -> ResourceLimits {
    let preset = if cfg!(debug_assertions) {
        ResourceLimits::development()
    } else {
        ResourceLimits::production()
    };
    ResourceLimits {
        max_connections: MAX_CONNECTIONS,
        max_requests_per_minute: MAX_REQUESTS_PER_MINUTE,
        max_requests_per_hour: MAX_REQUESTS_PER_HOUR,
        ..preset
    }
}

/// A [`ResourceManager`] enforcing [`resource_limits`].
#[must_use]
pub fn resource_manager() -> ResourceManager {
    ResourceManager::with_limits(resource_limits())
}
