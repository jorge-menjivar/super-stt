// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::types::SuperSTTDaemon;
use axum::extract::FromRef;
use parking_lot::RwLock as ParkingRwLock;
use std::collections::HashSet;
use std::sync::Arc;
use super_engine_daemon::auth::Auth;

pub(crate) use super_engine_daemon::http::PeerInfo;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) daemon: Arc<SuperSTTDaemon>,
    /// Session tokens and consent state; what the guards check requests
    /// against.
    pub(crate) auth: Auth,
    /// Registry HTTP client; shared across all handler invocations.
    pub(crate) registry_client: Arc<crate::registry::client::Client>,
    /// Set of `source` strings with an install currently in flight.
    /// Guards against duplicate concurrent installs for the same backend.
    pub(crate) install_inflight: Arc<ParkingRwLock<HashSet<String>>>,
}

impl AppState {
    /// Construct the application state from a daemon handle and its loaded
    /// auth state. The registry client is configured from environment
    /// variables.
    pub(crate) fn new(daemon: Arc<SuperSTTDaemon>, auth: Auth) -> Self {
        Self {
            daemon,
            auth,
            registry_client: Arc::new(crate::registry::client::Client::from_env()),
            install_inflight: Arc::new(ParkingRwLock::new(HashSet::new())),
        }
    }
}

/// Lets the guards and the auth routes extract the [`Auth`] alone.
impl FromRef<AppState> for Auth {
    fn from_ref(state: &AppState) -> Self {
        state.auth.clone()
    }
}
