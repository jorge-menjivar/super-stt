// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::internal::auth::consent::ConsentLocks;
use crate::daemon::http::internal::auth::middleware::DenyCache;
use crate::daemon::http::internal::auth::tokens::TokenStore;
use crate::daemon::types::SuperSTTDaemon;
use parking_lot::RwLock as ParkingRwLock;
use std::collections::HashSet;
use std::sync::Arc;

/// Per-connection extension carrying whatever the daemon knows about the
/// caller.
///
/// On the Unix socket that is peer credentials resolved at accept time via
/// `SO_PEERCRED`; `pid`/`uid` are `None` when the platform doesn't support
/// `peer_cred()`. On the TCP listener there are no credentials to read, and
/// [`Self::web_origin`] carries the caller's identity instead.
///
/// The two are mutually exclusive by construction: the Unix accept loop never
/// sets an origin, and the TCP one never has credentials to set. Which listener
/// a request arrived on is therefore readable from this struct alone, with no
/// separate transport flag that could disagree with it.
#[derive(Clone, Debug)]
pub(crate) struct PeerInfo {
    pub(crate) pid: Option<u32>,
    pub(crate) uid: Option<u32>,
    /// The allowlisted `Origin` of a TCP caller, `None` for a Unix one.
    ///
    /// Set only by
    /// [`require_allowed_origin`](crate::daemon::http::internal::auth::middleware::require_allowed_origin),
    /// which is the single place a request header becomes an identity. A value
    /// here has already been checked against
    /// [`TcpConfig::allowed_origins`](crate::config::TcpConfig::allowed_origins),
    /// so everything downstream can treat it as the user's own choice rather
    /// than as an attacker-supplied string.
    pub(crate) web_origin: Option<String>,
}

impl PeerInfo {
    /// The peer credentials of a caller on the Unix socket.
    #[must_use]
    pub(crate) fn unix(pid: Option<u32>, uid: Option<u32>) -> Self {
        Self {
            pid,
            uid,
            web_origin: None,
        }
    }

    /// A caller on the TCP listener, before its origin has been checked.
    ///
    /// The origin is attached later, by the gate that validates it — a
    /// connection starts out with no identity at all, which is what makes an
    /// un-gated route fail closed rather than fall back to something weaker.
    #[must_use]
    pub(crate) fn tcp() -> Self {
        Self {
            pid: None,
            uid: None,
            web_origin: None,
        }
    }

    /// Stable client identifier built from peer credentials, used as
    /// the key into [`ResourceManager`] for connection / rate-limit
    /// tracking. Falls back to `"unknown"` if neither uid nor pid is
    /// available — that bucket aggregates traffic from any peer whose
    /// credentials we couldn't resolve.
    ///
    /// A web caller is keyed by its origin, so two pages served from different
    /// origins get their own quota. Every page from one origin shares a bucket,
    /// which is the finest distinction available: a browser gives the daemon no
    /// per-tab identity, and inventing one from a header the page controls
    /// would just be a quota any page could reset at will.
    #[must_use]
    pub(crate) fn client_id(&self) -> String {
        if let Some(origin) = &self.web_origin {
            return format!("web:{origin}");
        }
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

#[cfg(test)]
mod client_id_tests {
    use super::PeerInfo;

    /// The rate limiter refuses any client id the resource manager has not seen,
    /// so the id a connection is registered under and the id looked up per
    /// request have to be the same string.
    ///
    /// A TCP caller is the awkward case: it is anonymous at accept time and only
    /// names itself once its `Origin` clears the allowlist. This asserts the two
    /// ids genuinely differ, which is why the origin gate has to register the
    /// second one — getting that wrong answered every browser request with a
    /// `429` about a quota that had never been counted.
    #[test]
    fn a_tcp_peer_changes_identity_once_its_origin_is_known() {
        let at_accept = PeerInfo::tcp();
        let mut once_gated = PeerInfo::tcp();
        once_gated.web_origin = Some("http://localhost:8910".to_string());

        assert_eq!(at_accept.client_id(), "unknown");
        assert_eq!(once_gated.client_id(), "web:http://localhost:8910");
        assert_ne!(
            at_accept.client_id(),
            once_gated.client_id(),
            "if these ever match, the origin gate's registration is redundant; \
             while they differ, it is what keeps the rate limiter working"
        );
    }

    /// Two origins are two clients, so one page cannot spend another's quota.
    #[test]
    fn each_origin_gets_its_own_bucket() {
        let mut a = PeerInfo::tcp();
        a.web_origin = Some("http://localhost:8910".to_string());
        let mut b = PeerInfo::tcp();
        b.web_origin = Some("https://example.test".to_string());
        assert_ne!(a.client_id(), b.client_id());
    }

    /// A Unix peer keeps the uid:pid key it always had — the web branch must not
    /// have changed the identity of callers that were already working.
    #[test]
    fn a_unix_peer_is_still_keyed_by_uid_and_pid() {
        assert_eq!(PeerInfo::unix(Some(42), Some(1000)).client_id(), "1000:42");
        assert_eq!(PeerInfo::unix(None, Some(1000)).client_id(), "1000:?");
        assert_eq!(PeerInfo::unix(Some(42), None).client_id(), "?:42");
    }
}
