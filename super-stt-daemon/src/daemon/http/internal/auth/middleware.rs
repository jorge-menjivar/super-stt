// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::internal::auth::consent::ConsentKey;
use crate::daemon::http::internal::auth::tokens::TokenMeta;
use crate::daemon::http::internal::helpers::responses::{
    invalid_session, rate_limited, reason, scope_denied,
};
use crate::daemon::http::state::{AppState, PeerInfo};
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::Response;
use std::sync::{Arc, Mutex};

/// In-memory record of `(exe_path, scopes)` pairs the user has clicked
/// Deny on. Subsequent `/auth/request` calls for the same pair
/// short-circuit to `403 auth_denied` without spawning another
/// consent popup — the user already said no, no point asking again.
///
/// **In-memory only.** The set lives for the daemon's lifetime; a
/// daemon restart resets it so the user gets a fresh chance to grant
/// consent if they want to. This intentionally has no keyring/disk
/// persistence (per spec).
#[derive(Clone, Default)]
pub(crate) struct DenyCache {
    pub(crate) inner: Arc<Mutex<std::collections::HashSet<ConsentKey>>>,
}

impl DenyCache {
    pub(crate) fn contains(&self, key: &ConsentKey) -> bool {
        self.inner.lock().unwrap().contains(key)
    }

    pub(crate) fn insert(&self, key: ConsentKey) {
        self.inner.lock().unwrap().insert(key);
    }
}

pub(crate) fn extract_bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::to_owned)
}

/// The validated session metadata + bearer token, attached to each
/// authorized request as an `axum::Extension` so handlers can read them
/// without re-validating. The bearer string is included so handlers
/// like `/events` can call back into `TokenStore` to revoke on
/// `exe_changed`.
#[derive(Clone, Debug)]
pub(crate) struct AuthContext {
    pub(crate) meta: TokenMeta,
    pub(crate) token: String,
}

/// Validate the bearer token and require that its granted scope set
/// contains `required`. Attaches the [`AuthContext`] on success so the
/// handler can read the scopes/exe without re-validating.
async fn require_scope(
    required: &str,
    state: AppState,
    headers: HeaderMap,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let Some(token) = extract_bearer_token(&headers) else {
        return invalid_session(reason::UNKNOWN);
    };
    match state.tokens.validate(&token) {
        Ok(meta) => {
            if meta.scopes.iter().any(|s| s == required) {
                request.extensions_mut().insert(AuthContext { meta, token });
                next.run(request).await
            } else {
                scope_denied()
            }
        }
        Err(reason) => invalid_session(reason),
    }
}

pub(crate) async fn require_transcribe_scope(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Response {
    require_scope("transcribe", state, headers, request, next).await
}

pub(crate) async fn require_status_scope(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Response {
    require_scope("status", state, headers, request, next).await
}

pub(crate) async fn require_settings_scope(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Response {
    require_scope("settings", state, headers, request, next).await
}

pub(crate) async fn require_secrets_scope(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Response {
    require_scope("secrets", state, headers, request, next).await
}

/// Accept any valid bearer token regardless of scope. Used for `/ping`
/// — a no-info-leak liveness probe that all scopes legitimately need.
pub(crate) async fn require_any_authenticated(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let Some(token) = extract_bearer_token(&headers) else {
        return invalid_session(reason::UNKNOWN);
    };
    match state.tokens.validate(&token) {
        Ok(meta) => {
            request.extensions_mut().insert(AuthContext { meta, token });
            next.run(request).await
        }
        Err(reason) => invalid_session(reason),
    }
}

/// Per-request rate-limit gate. Layered on every authenticated
/// route group — `/auth/request` is excluded because its abuse
/// model is the consent popup, not per-request quota.
pub(crate) async fn require_rate_limit(
    State(state): State<AppState>,
    axum::Extension(peer): axum::Extension<PeerInfo>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let client_id = peer.client_id();
    match state
        .daemon
        .resource_manager
        .record_request(&client_id)
        .await
    {
        Ok(()) => next.run(request).await,
        Err(e) => {
            log::warn!("rate-limit hit for {client_id}: {e}");
            rate_limited()
        }
    }
}

#[cfg(test)]
mod tests {
    //! Deny-cache identity. The cache short-circuits `/auth/request` to
    //! `403 auth_denied (user_denied_cached)` so a binary the user
    //! already rejected can't re-trigger the consent popup. Its key is
    //! the `(exe_path, scopes)` pair — the same identity the consent
    //! flow is verified against — so denial must be scoped to that exact
    //! binary and that exact scope set, nothing broader.
    use super::DenyCache;
    use crate::daemon::http::internal::auth::consent::ConsentKey;
    use std::path::PathBuf;

    #[test]
    fn deny_cache_remembers_a_denied_pair() {
        let cache = DenyCache::default();
        let key: ConsentKey = (
            PathBuf::from("/usr/bin/evil"),
            vec!["settings".to_string(), "transcribe".to_string()],
        );
        assert!(!cache.contains(&key), "a fresh cache denies nothing");
        cache.insert(key.clone());
        assert!(
            cache.contains(&key),
            "a denied (exe, scopes) pair must be remembered"
        );
    }

    #[test]
    fn deny_cache_is_scoped_to_exe_and_scope_set() {
        let cache = DenyCache::default();
        let denied: ConsentKey = (PathBuf::from("/usr/bin/evil"), vec!["settings".to_string()]);
        cache.insert(denied.clone());

        // Same scopes, different binary → not denied (a fresh consent prompt).
        let other_exe: ConsentKey = (PathBuf::from("/usr/bin/other"), denied.1.clone());
        assert!(
            !cache.contains(&other_exe),
            "denial must not leak across binaries"
        );

        // Same binary, different scope set → not denied.
        let other_scopes: ConsentKey = (denied.0.clone(), vec!["status".to_string()]);
        assert!(
            !cache.contains(&other_scopes),
            "denial must not leak across scope sets"
        );
    }
}
