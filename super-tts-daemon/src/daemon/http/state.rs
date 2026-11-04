// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::types::SuperTTSDaemon;
use axum::extract::FromRef;
use std::sync::Arc;
use super_engine_daemon::auth::Auth;
use super_engine_daemon::registry::endpoints::Registry;

pub(crate) use super_engine_daemon::http::PeerInfo;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) daemon: Arc<SuperTTSDaemon>,
    /// Session tokens and consent state; what the guards check requests
    /// against.
    pub(crate) auth: Auth,
    /// The registry endpoints' index client and the installs in flight.
    pub(crate) registry: Registry<SuperTTSDaemon>,
}

impl AppState {
    /// Construct the application state from a daemon handle and its loaded
    /// auth state. The registry client is configured from environment
    /// variables.
    pub(crate) fn new(daemon: Arc<SuperTTSDaemon>, auth: Auth) -> Self {
        let registry = Registry::new(
            Arc::clone(&daemon),
            crate::registry::client::Client::from_env(crate::registry::DAEMON),
        );
        Self {
            daemon,
            auth,
            registry,
        }
    }
}

/// Lets the guards and the auth routes extract the [`Auth`] alone.
impl FromRef<AppState> for Auth {
    fn from_ref(state: &AppState) -> Self {
        state.auth.clone()
    }
}
