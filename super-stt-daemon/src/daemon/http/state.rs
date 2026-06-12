// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::internal::auth::consent::ConsentLocks;
use crate::daemon::http::internal::auth::middleware::DenyCache;
use crate::daemon::http::internal::auth::tokens::TokenStore;
use crate::daemon::types::SuperSTTDaemon;
use parking_lot::RwLock as ParkingRwLock;
use std::collections::HashSet;
use std::sync::Arc;

/// Per-connection extension carrying peer credentials resolved at
/// accept time via `SO_PEERCRED`. Fields are `None` when the
/// platform doesn't support `peer_cred()`.
#[derive(Clone, Debug)]
pub(crate) struct PeerInfo {
    pub(crate) pid: Option<u32>,
    pub(crate) uid: Option<u32>,
}

impl PeerInfo {
    /// Stable client identifier built from peer credentials, used as
    /// the key into [`ResourceManager`] for connection / rate-limit
    /// tracking. Falls back to `"unknown"` if neither uid nor pid is
    /// available — that bucket aggregates traffic from any peer whose
    /// credentials we couldn't resolve.
    #[must_use]
    pub(crate) fn client_id(&self) -> String {
        match (self.uid, self.pid) {
            (Some(uid), Some(pid)) => format!("{uid}:{pid}"),
            (Some(uid), None) => format!("{uid}:?"),
            (None, Some(pid)) => format!("?:{pid}"),
            (None, None) => "unknown".to_string(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) daemon: Arc<SuperSTTDaemon>,
    pub(crate) tokens: TokenStore,
    pub(crate) consent_locks: ConsentLocks,
    pub(crate) deny_cache: DenyCache,
    /// Registry HTTP client; shared across all handler invocations.
    pub(crate) registry_client: Arc<crate::registry::client::Client>,
    /// Set of `source` strings with an install currently in flight.
    /// Guards against duplicate concurrent installs for the same backend.
    pub(crate) install_inflight: Arc<ParkingRwLock<HashSet<String>>>,
}

impl AppState {
    /// Construct the default application state from a daemon handle.
    /// The registry client is configured from environment variables.
    ///
    /// # Errors
    /// Returns an error if the system keyring is unavailable while loading
    /// the persisted session store, so the daemon refuses to start. See
    /// [`TokenStore::load_persisted`].
    pub(crate) fn new(daemon: Arc<SuperSTTDaemon>) -> anyhow::Result<Self> {
        Ok(Self {
            daemon,
            tokens: TokenStore::load_persisted()?,
            consent_locks: ConsentLocks::default(),
            deny_cache: DenyCache::default(),
            registry_client: Arc::new(crate::registry::client::Client::from_env()),
            install_inflight: Arc::new(ParkingRwLock::new(HashSet::new())),
        })
    }
}
