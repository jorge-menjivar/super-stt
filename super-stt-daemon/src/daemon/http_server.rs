// SPDX-License-Identifier: GPL-3.0-only
//! HTTP server for the new daemon protocol.
//!
//! Runs **side-by-side** with the legacy length-prefix Unix-socket listener
//! (in `client_management.rs` / `types.rs::start`). The legacy listener stays
//! exactly as-is; this module only adds a second listener on a separate
//! socket path (`$XDG_RUNTIME_DIR/stt/super-stt-http.sock`).
//!
//! v1 endpoint set:
//!
//! - `POST /auth/request`     — interactive consent → mints a session token
//! - `GET  /ping`             — liveness (requires Bearer token)
//! - `GET  /status`           — current model + device (requires Bearer token)
//! - `POST /transcribe`       — start a daemon-mic recording (requires Bearer token)
//! - `POST /transcribe/stop`  — stop an in-flight daemon-mic recording (requires Bearer token)
//!
//! Authentication:
//! - The daemon uses `SO_PEERCRED` on each connection to get the peer PID
//!   and resolves `/proc/<pid>/exe`. That path is shown in the consent
//!   popup so the user knows which binary is asking.
//! - On Allow, the daemon mints a 32-byte hex session token and stores it
//!   keyed in an in-memory `TokenStore`. The token has a 30-day expiry.
//! - Every endpoint other than `/auth/request` requires
//!   `Authorization: Bearer <token>`. Missing/invalid → 401 with
//!   `{ status: "error", message: "invalid_session", data: { reason } }`.
//! - The popup is the `super-stt-consent` helper binary, spawned as a
//!   subprocess. It writes "allow" / "deny" / "dismissed" to stdout.
//! - Set `SUPER_STT_AUTO_APPROVE=1` in the daemon environment to skip
//!   the popup entirely (intended for tests / CI).
//!
//! Token persistence to the system keyring is a follow-up; v1 keeps
//! tokens in memory only.

use crate::daemon::types::SuperSTTDaemon;
use anyhow::{Context, Result};
use axum::Router;
use axum::body::Body;
use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use log::{info, warn};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use super_stt_shared::models::protocol::{DaemonRequest, DaemonResponse};
use tokio::net::UnixListener;
use tokio::sync::broadcast;

/// Env var that, when set to "1", bypasses the consent popup entirely
/// and auto-approves every `auth_request`. Intended for tests / CI only.
pub const AUTO_APPROVE_ENV: &str = "SUPER_STT_AUTO_APPROVE";

/// Schema version for the persisted sessions blob. Bump on any breaking
/// change to `TokenMeta`'s on-disk shape so an older daemon can refuse
/// to load a newer file rather than misinterpret fields.
const SESSIONS_SCHEMA_VERSION: u32 = 1;

/// After a keyring write fails, suppress further attempts for this
/// long. A locked keyring would otherwise re-prompt the user every
/// time a session is minted, expired, or revoked. Cleared by the next
/// successful write.
const KEYRING_FAILURE_COOLDOWN: Duration = Duration::from_mins(5);

/// Tracks the most recent keyring-write failure timestamp so
/// `flush_locked` can short-circuit during the cooldown window.
static KEYRING_LAST_FAILURE: std::sync::LazyLock<Mutex<Option<std::time::Instant>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

fn keyring_writes_in_cooldown() -> bool {
    KEYRING_LAST_FAILURE
        .lock()
        .unwrap()
        .is_some_and(|t| t.elapsed() < KEYRING_FAILURE_COOLDOWN)
}

fn mark_keyring_failure() {
    *KEYRING_LAST_FAILURE.lock().unwrap() = Some(std::time::Instant::now());
}

fn clear_keyring_failure_flag() {
    *KEYRING_LAST_FAILURE.lock().unwrap() = None;
}

/// Persistent store of issued session tokens. The in-memory `HashMap`
/// is the hot lookup path; every mutation also writes the whole map
/// back to the system keyring under `(super-stt, stt-sessions)` so a
/// daemon restart re-hydrates the same set of valid tokens.
#[derive(Clone, Default)]
pub struct TokenStore {
    inner: Arc<Mutex<HashMap<String, TokenMeta>>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[allow(dead_code)] // app_name / scope / exe_path are wired for future scope-aware dispatch
struct TokenMeta {
    app_name: String,
    scope: String,
    exe_path: PathBuf,
    issued_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

/// On-disk wrapper for the sessions map. Keyed by token so the JSON
/// shape mirrors the in-memory `HashMap` exactly.
#[derive(Serialize, Deserialize)]
struct SessionsFile {
    version: u32,
    sessions: HashMap<String, TokenMeta>,
}

impl TokenStore {
    /// Load any persisted sessions from the keyring, prune anything
    /// already past its `expires_at`, and write the cleaned set back if
    /// pruning removed entries. Failure at any step (missing entry,
    /// keyring unavailable, parse error, version mismatch) yields an
    /// empty store — the daemon must not refuse to start because the
    /// keyring is unhappy.
    fn load_persisted() -> Self {
        let store = Self::default();

        let blob = match crate::keyring::get_sessions_blob() {
            Ok(Some(b)) => b,
            Ok(None) => {
                info!("No persisted sessions found; starting with empty store");
                return store;
            }
            Err(e) => {
                warn!("Failed to read persisted sessions ({e}); starting with empty store");
                return store;
            }
        };

        let parsed: SessionsFile = match serde_json::from_str(&blob) {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to parse persisted sessions ({e}); starting with empty store");
                return store;
            }
        };

        if parsed.version != SESSIONS_SCHEMA_VERSION {
            warn!(
                "Persisted sessions schema version {} != expected {}; ignoring",
                parsed.version, SESSIONS_SCHEMA_VERSION
            );
            return store;
        }

        let now = Utc::now();
        let total = parsed.sessions.len();
        let live: HashMap<String, TokenMeta> = parsed
            .sessions
            .into_iter()
            .filter(|(_, meta)| meta.expires_at > now)
            .collect();
        let pruned = total - live.len();

        info!(
            "Loaded {} persisted sessions ({pruned} expired pruned)",
            live.len()
        );

        if pruned > 0 {
            // Write the cleaned map back so disk state matches memory.
            let cleaned = SessionsFile {
                version: SESSIONS_SCHEMA_VERSION,
                sessions: live.clone(),
            };
            if let Ok(json) = serde_json::to_string(&cleaned)
                && let Err(e) = crate::keyring::set_sessions_blob(&json)
            {
                warn!("Failed to persist pruned sessions blob: {e}");
            }
        }

        *store.inner.lock().unwrap() = live;
        store
    }

    /// Persist the current sessions map to the keyring. Callers must
    /// pass the locked guard so we can flush under the same lock that
    /// guards the in-memory map (no torn writes vs concurrent mints).
    /// Failures are logged but not propagated — the in-memory state is
    /// still authoritative for the lifetime of the daemon.
    ///
    /// **Failure suppression.** A locked or denied keyring will fail
    /// every write; without a cooldown, a busy session-mint loop would
    /// re-prompt the user every few seconds. After one failure we
    /// suppress further writes for `KEYRING_FAILURE_COOLDOWN`. The
    /// daemon's in-memory map remains correct; we just lose
    /// persistence for that window. A subsequent successful write
    /// (e.g. user unlocks the keyring) clears the suppression flag.
    ///
    /// In `cargo test` builds this is a no-op so unit tests don't
    /// pollute or depend on the developer's real keyring. End-to-end
    /// behavior across daemon restarts is covered by the integration
    /// smoke test in `tests/http_smoke_full.rs` (which exercises this
    /// crate built without the `test` cfg flag).
    fn flush_locked(map: &HashMap<String, TokenMeta>) {
        if cfg!(test) {
            return;
        }
        if keyring_writes_in_cooldown() {
            // Suppressed — last write failed within the cooldown
            // window. In-memory state is still authoritative.
            return;
        }
        let payload = SessionsFile {
            version: SESSIONS_SCHEMA_VERSION,
            // Cheap clone — TokenMeta is small. Avoids holding a
            // borrow across the keyring write so callers can drop the
            // guard right after.
            sessions: map.clone(),
        };
        let json = match serde_json::to_string(&payload) {
            Ok(j) => j,
            Err(e) => {
                warn!("Failed to serialize sessions blob: {e}");
                return;
            }
        };
        match crate::keyring::set_sessions_blob(&json) {
            Ok(()) => {
                clear_keyring_failure_flag();
            }
            Err(e) => {
                warn!(
                    "Failed to persist sessions blob: {e}; suppressing further keyring writes for {}s",
                    KEYRING_FAILURE_COOLDOWN.as_secs()
                );
                mark_keyring_failure();
            }
        }
    }

    fn mint(&self, app_name: &str, scope: &str, exe_path: &Path) -> (String, DateTime<Utc>) {
        let token = generate_token();
        let now = Utc::now();
        let expires_at = now + ChronoDuration::days(30);
        let meta = TokenMeta {
            app_name: app_name.to_string(),
            scope: scope.to_string(),
            exe_path: exe_path.to_path_buf(),
            issued_at: now,
            expires_at,
        };
        let mut tokens = self.inner.lock().unwrap();
        tokens.insert(token.clone(), meta);
        Self::flush_locked(&tokens);
        (token, expires_at)
    }

    fn validate(&self, token: &str) -> Result<TokenMeta, &'static str> {
        let mut tokens = self.inner.lock().unwrap();
        let meta = tokens.get(token).ok_or("unknown")?.clone();
        if meta.expires_at < Utc::now() {
            tokens.remove(token);
            Self::flush_locked(&tokens);
            return Err("expired");
        }
        Ok(meta)
    }

    /// Drop a session token immediately and persist the change. Used by
    /// the `/events` exe-watch path on `exe_changed`: a binary
    /// replacement during a long-lived widget connection invalidates
    /// the session, so the daemon revokes the token, emits a `revoked`
    /// SSE event, and closes the stream. Idempotent.
    fn revoke(&self, token: &str) {
        let mut tokens = self.inner.lock().unwrap();
        if tokens.remove(token).is_some() {
            Self::flush_locked(&tokens);
        }
    }
}

fn generate_token() -> String {
    use std::fmt::Write as _;
    let rng = SystemRandom::new();
    let mut bytes = [0u8; 32];
    rng.fill(&mut bytes).expect("SystemRandom::fill");
    let mut s = String::with_capacity(64);
    for b in bytes {
        write!(&mut s, "{b:02x}").expect("string write");
    }
    s
}

/// Per-connection extension carrying the peer PID resolved at accept time
/// via `SO_PEERCRED`. None when the platform doesn't support it.
#[derive(Clone, Debug)]
pub struct PeerInfo {
    pub pid: Option<u32>,
}

/// Identifies the consent flow uniquely: (`app_name`, `exe_path`,
/// `scope`). Two requests with the same key share a single
/// popup-and-mint cycle.
type ConsentKey = (String, PathBuf, String);
type ConsentLock = Arc<tokio::sync::Mutex<()>>;

/// Per-`(app_name, exe_path, scope)` async mutex registry used by the
/// `/auth/request` handler to dedup concurrent first-time consent
/// requests. Without this, two clients that ping the daemon at the same
/// time on a fresh install would each spawn their own consent popup;
/// with it, the second blocks until the first finishes and then
/// short-circuits via the reuse-scan against the now-minted token.
#[derive(Clone, Default)]
struct ConsentLocks {
    inner: Arc<Mutex<HashMap<ConsentKey, ConsentLock>>>,
}

impl ConsentLocks {
    fn lock_for(&self, key: ConsentKey) -> ConsentLock {
        let mut map = self.inner.lock().unwrap();
        map.entry(key)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }
}

/// In-memory record of `(app_name, exe_path, scope)` triples the user
/// has clicked Deny on. Subsequent `/auth/request` calls for the same
/// triple short-circuit to `403 auth_denied` without spawning another
/// consent popup — the user already said no, no point asking again.
///
/// **In-memory only.** The set lives for the daemon's lifetime; a
/// daemon restart resets it so the user gets a fresh chance to grant
/// consent if they want to. This intentionally has no keyring/disk
/// persistence (per spec).
#[derive(Clone, Default)]
struct DenyCache {
    inner: Arc<Mutex<std::collections::HashSet<ConsentKey>>>,
}

impl DenyCache {
    fn contains(&self, key: &ConsentKey) -> bool {
        self.inner.lock().unwrap().contains(key)
    }

    fn insert(&self, key: ConsentKey) {
        self.inner.lock().unwrap().insert(key);
    }
}

#[derive(Clone)]
struct AppState {
    daemon: Arc<SuperSTTDaemon>,
    tokens: TokenStore,
    consent_locks: ConsentLocks,
    deny_cache: DenyCache,
}

/// Wire up the router with all endpoints, scope-aware middleware, and
/// state. Endpoint groups split by required scope:
///
/// - `/auth/request`: reachable WITHOUT a token (it's how you get one).
/// - client-scope: any valid token (client OR settings).
/// - settings-scope: only valid `settings` tokens. Client tokens get
///   403 `scope_denied` here.
/// - widget-scope: `widget` or `settings` tokens (settings is god-mode).
///   Hosts the long-lived `GET /events` SSE subscription.
fn build_router(state: AppState) -> Router {
    let client_scope = Router::new()
        .route("/ping", get(ping))
        .route("/status", get(status))
        .route("/transcribe", post(transcribe))
        .route("/transcribe/stop", post(transcribe_stop))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_client_scope,
        ));

    let widget_scope =
        Router::new()
            .route("/events", get(events))
            .layer(middleware::from_fn_with_state(
                state.clone(),
                require_widget_scope,
            ));

    let settings_scope = Router::new()
        // Active model
        .route(
            "/active_model",
            get(get_active_model).post(set_active_model),
        )
        .route("/active_model/cancel", post(cancel_set_active_model))
        .route("/models", get(list_models))
        // Active device
        .route(
            "/active_device",
            get(get_active_device).post(set_active_device),
        )
        // Audio theme
        .route("/audio_theme", get(get_audio_theme).post(set_audio_theme))
        .route("/audio_theme/test", post(test_audio_theme))
        .route("/audio_themes", get(list_audio_themes))
        // Volume
        .route("/volume", get(get_volume).post(set_volume))
        // Recording stop mode
        .route(
            "/recording_stop_mode",
            get(get_recording_stop_mode).post(set_recording_stop_mode),
        )
        // Write method
        .route(
            "/write_method",
            get(get_write_method).post(set_write_method),
        )
        // Preview typing
        .route(
            "/preview_typing",
            get(get_preview_typing).post(set_preview_typing),
        )
        // Online models gate
        .route(
            "/allow_online_models",
            get(get_allow_online_models).post(set_allow_online_models),
        )
        // Custom models dir
        .route(
            "/custom_models_dir",
            get(get_custom_models_dir).post(set_custom_models_dir),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_settings_scope,
        ));

    Router::new()
        .merge(client_scope)
        .merge(settings_scope)
        .merge(widget_scope)
        .route("/auth/request", post(auth_request))
        .with_state(state)
}

/// Spawn the HTTP server on the dedicated Unix socket. Returns once the
/// listener is bound (the actual accept loop runs in a background task and
/// terminates when `shutdown_tx` fires).
///
/// # Errors
/// Returns an error if the socket can't be created or bound.
pub async fn start_http_server(
    daemon: Arc<SuperSTTDaemon>,
    socket_path: PathBuf,
    shutdown_tx: broadcast::Sender<()>,
) -> Result<()> {
    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("Failed to create http socket directory")?;
    }
    if socket_path.exists() {
        tokio::fs::remove_file(&socket_path)
            .await
            .context("Failed to remove existing http socket file")?;
    }

    let listener = UnixListener::bind(&socket_path).context("Failed to bind http Unix socket")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if cfg!(debug_assertions) { 0o666 } else { 0o660 };
        let perms = std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(&socket_path, perms)
            .context("Failed to set http socket permissions")?;
    }

    info!(
        "HTTP daemon listening on socket: {} (side-by-side with legacy listener)",
        socket_path.display()
    );

    let state = AppState {
        daemon: Arc::clone(&daemon),
        tokens: TokenStore::load_persisted(),
        consent_locks: ConsentLocks::default(),
        deny_cache: DenyCache::default(),
    };

    let app = build_router(state);

    let cleanup_path = socket_path.clone();
    tokio::spawn(async move {
        let mut shutdown_rx = shutdown_tx.subscribe();
        let server_loop = async {
            loop {
                let (stream, _addr) = match listener.accept().await {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("http accept failed: {e}");
                        continue;
                    }
                };
                #[allow(clippy::cast_sign_loss)]
                let peer_pid = stream
                    .peer_cred()
                    .ok()
                    .and_then(|c| c.pid())
                    .map(|p| p as u32);
                let app_for_conn = app
                    .clone()
                    .layer(axum::Extension(PeerInfo { pid: peer_pid }));
                tokio::spawn(async move {
                    let io = hyper_util::rt::TokioIo::new(stream);
                    let svc = hyper_util::service::TowerToHyperService::new(app_for_conn);
                    if let Err(e) = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, svc)
                        .await
                    {
                        log::debug!("http connection finished: {e}");
                    }
                });
            }
        };
        tokio::select! {
            _ = server_loop => {}
            _ = shutdown_rx.recv() => {
                info!("HTTP server shutting down");
            }
        }
        if cleanup_path.exists() {
            let _ = tokio::fs::remove_file(&cleanup_path).await;
        }
    });

    Ok(())
}

// -----------------------------------------------------------------------------
// Bearer-token + scope middleware
// -----------------------------------------------------------------------------

/// Scope required by an endpoint group.
#[derive(Clone, Copy, Debug)]
enum RequiredScope {
    Client,
    Settings,
    Widget,
}

fn extract_bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::to_owned)
}

fn scope_satisfies(have: &str, required: RequiredScope) -> bool {
    match (have, required) {
        // settings is god-mode (can satisfy any scope tier);
        // client tokens only fit client-scope endpoints;
        // widget tokens only fit widget-scope endpoints.
        ("settings", _) | ("client", RequiredScope::Client) | ("widget", RequiredScope::Widget) => {
            true
        }
        _ => false,
    }
}

/// The validated session metadata + bearer token, attached to each
/// authorized request as an `axum::Extension` so handlers can read them
/// without re-validating. The bearer string is included so handlers
/// like `/events` can call back into `TokenStore` to revoke on
/// `exe_changed`.
#[derive(Clone, Debug)]
struct AuthContext {
    meta: TokenMeta,
    token: String,
}

async fn require_scope(
    required: RequiredScope,
    state: AppState,
    headers: HeaderMap,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let Some(token) = extract_bearer_token(&headers) else {
        return invalid_session("unknown");
    };
    match state.tokens.validate(&token) {
        Ok(meta) => {
            if scope_satisfies(&meta.scope, required) {
                request.extensions_mut().insert(AuthContext { meta, token });
                next.run(request).await
            } else {
                scope_denied()
            }
        }
        Err(reason) => invalid_session(reason),
    }
}

async fn require_client_scope(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Response {
    require_scope(RequiredScope::Client, state, headers, request, next).await
}

async fn require_settings_scope(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Response {
    require_scope(RequiredScope::Settings, state, headers, request, next).await
}

async fn require_widget_scope(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Response {
    require_scope(RequiredScope::Widget, state, headers, request, next).await
}

fn invalid_session(reason: &'static str) -> Response {
    let body = serde_json::json!({
        "status":  "error",
        "message": "invalid_session",
        "data":    { "reason": reason }
    });
    (
        StatusCode::UNAUTHORIZED,
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

fn scope_denied() -> Response {
    let body = serde_json::json!({
        "status":  "error",
        "message": "scope_denied",
    });
    (
        StatusCode::FORBIDDEN,
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

// -----------------------------------------------------------------------------
// /auth/request
// -----------------------------------------------------------------------------

#[derive(Deserialize, Debug)]
struct AuthRequestBody {
    app_name: String,
    scope: String,
    #[serde(default)]
    #[allow(dead_code)] // accepted for forwards compat; not yet used
    version: Option<String>,
}

#[derive(Serialize)]
struct AuthOk<'a> {
    status: &'a str,
    session_token: String,
    scope: &'a str,
    expires_at: String,
}

#[derive(Serialize)]
struct AuthErr<'a> {
    status: &'a str,
    message: &'a str,
    data: AuthErrData<'a>,
}

#[derive(Serialize)]
struct AuthErrData<'a> {
    reason: &'a str,
}

async fn auth_request(
    State(state): State<AppState>,
    peer: Option<axum::Extension<PeerInfo>>,
    body: Option<axum::Json<AuthRequestBody>>,
) -> Response {
    let Some(axum::Json(body)) = body else {
        return auth_err(StatusCode::BAD_REQUEST, "auth_denied", "invalid_body");
    };

    if !matches!(body.scope.as_str(), "client" | "settings" | "widget") {
        return auth_err(StatusCode::BAD_REQUEST, "auth_denied", "invalid_scope");
    }

    // Resolve the calling binary via /proc/<pid>/exe. Log each
    // failure mode separately so we can tell whether the issue is
    // missing peer credentials, a missing pid, or a kernel/proc
    // permission denial. Falls back to "<unknown>" so the popup still
    // has something to display.
    let exe_path = resolve_peer_exe(peer.as_ref());

    // Helper to build the success response for a (token, expires_at)
    // pair. Used by both reuse-scan paths (fast and slow) and the
    // post-mint path so they all serialize the same JSON shape.
    let ok_response = |token: String, expires_at: DateTime<Utc>| -> Response {
        let payload = AuthOk {
            status: "success",
            session_token: token,
            scope: scope_str(&body.scope),
            expires_at: expires_at.to_rfc3339(),
        };
        (
            StatusCode::OK,
            [("content-type", "application/json")],
            serde_json::to_string(&payload).unwrap_or_default(),
        )
            .into_response()
    };

    // `auth_request` always runs the consent flow — no token
    // validation, no identity-only reuse. Clients that have a valid
    // cached token never reach this endpoint; they go straight to
    // `/ping`/`/events`/etc., where the bearer header is validated
    // by `TokenStore::validate`. Clients that hit `auth_request`
    // do so precisely because they need a fresh token, and the
    // consistent semantic is "request fresh consent".

    let consent_key = (body.app_name.clone(), exe_path.clone(), body.scope.clone());

    // Sticky-deny short-circuit. If the user previously clicked Deny
    // for this exact triple in this daemon's lifetime, reject
    // immediately without ever spawning another popup. The cache is
    // cleared by daemon restart; there's no other reset path on
    // purpose.
    if state.deny_cache.contains(&consent_key) {
        log::info!(
            "auth_request denied from cache (user previously denied): app={} scope={}",
            body.app_name,
            body.scope
        );
        return auth_err(StatusCode::FORBIDDEN, "auth_denied", "user_denied_cached");
    }

    // Serialize concurrent first-time requests for the same identity
    // so we don't spawn N consent popups when N clients race. Each
    // racer still gets its own popup, but they happen one at a time
    // rather than stacking on screen. (We no longer post-lock reuse
    // an identity-only match — see the rationale on
    // `find_session_by_token_and_identity` above.)
    let lock = state.consent_locks.lock_for(consent_key.clone());
    let _guard = lock.lock().await;

    // Re-check the deny cache: a concurrent caller for the same
    // triple may have been denied while we were queued behind their
    // popup.
    if state.deny_cache.contains(&consent_key) {
        log::info!(
            "auth_request denied from cache (concurrent caller denied): app={} scope={}",
            body.app_name,
            body.scope
        );
        return auth_err(StatusCode::FORBIDDEN, "auth_denied", "user_denied_cached");
    }

    // Auto-approve if the daemon is in test/CI mode.
    let auto_approve = std::env::var(AUTO_APPROVE_ENV).is_ok_and(|v| v == "1");

    let decision = if auto_approve {
        log::info!(
            "{AUTO_APPROVE_ENV}=1 set; auto-approving auth_request for {} ({})",
            body.app_name,
            exe_path.display()
        );
        ConsentDecision::Allow
    } else {
        ask_user_for_consent(&body.app_name, &body.scope, &exe_path).await
    };

    finalize_consent_decision(
        decision,
        &state,
        &body,
        &exe_path,
        consent_key,
        &ok_response,
    )
}

/// Resolve a [`ConsentDecision`] into the matching HTTP response,
/// folding in the side effects each branch needs (token mint on
/// Allow, deny-cache insert on Deny). Extracted out of `auth_request`
/// to keep that handler under the workspace's clippy line cap.
fn finalize_consent_decision(
    decision: ConsentDecision,
    state: &AppState,
    body: &AuthRequestBody,
    exe_path: &Path,
    consent_key: ConsentKey,
    ok_response: &dyn Fn(String, DateTime<Utc>) -> Response,
) -> Response {
    match decision {
        ConsentDecision::Allow => {
            let (token, expires_at) = state.tokens.mint(&body.app_name, &body.scope, exe_path);
            log::info!(
                "auth_request approved: app={} scope={}",
                body.app_name,
                body.scope
            );
            ok_response(token, expires_at)
        }
        ConsentDecision::Deny => {
            // Sticky: remember this triple so the next request from
            // the same binary is auto-denied without re-prompting.
            // In-memory only — daemon restart resets.
            log::info!(
                "auth_request denied by user; caching deny for app={} scope={}",
                body.app_name,
                body.scope
            );
            state.deny_cache.insert(consent_key);
            auth_err(StatusCode::FORBIDDEN, "auth_denied", "user_denied")
        }
        ConsentDecision::Dismissed => {
            // *Don't* cache Dismissed (e.g. user closed the popup
            // without making a choice, the helper crashed, etc.).
            // Treat as transient — the next request gets a fresh
            // popup.
            auth_err(StatusCode::FORBIDDEN, "auth_denied", "user_dismissed")
        }
        ConsentDecision::PopupFailed => {
            auth_err(StatusCode::FORBIDDEN, "auth_denied", "popup_failed")
        }
    }
}

fn auth_err(status: StatusCode, message: &str, reason: &str) -> Response {
    let payload = AuthErr {
        status: "error",
        message,
        data: AuthErrData { reason },
    };
    (
        status,
        [("content-type", "application/json")],
        serde_json::to_string(&payload).unwrap_or_default(),
    )
        .into_response()
}

fn scope_str(s: &str) -> &'static str {
    match s {
        "settings" => "settings",
        "widget" => "widget",
        _ => "client",
    }
}

/// Resolve the calling process's executable path from the
/// `axum::Extension<PeerInfo>` attached by the accept loop. Returns
/// the canonical path on success, `PathBuf::from("<unknown>")` on any
/// failure — and **logs the specific reason** so we can tell, from
/// the journal, whether the issue is missing peer credentials
/// (`SO_PEERCRED` not supported, peer process gone), a missing pid in
/// the credential struct, or a kernel-level denial of the
/// `/proc/<pid>/exe` readlink (Yama `ptrace_scope`, systemd
/// `ProtectProc=`, sandboxed daemon, etc.).
fn resolve_peer_exe(peer: Option<&axum::Extension<PeerInfo>>) -> PathBuf {
    let Some(peer) = peer else {
        log::warn!(
            "auth_request: no PeerInfo extension attached — daemon won't be able to identify the requesting binary"
        );
        return PathBuf::from("<unknown>");
    };
    let Some(pid) = peer.0.pid else {
        log::warn!(
            "auth_request: PeerInfo had no pid (SO_PEERCRED returned no credentials); cannot resolve exe"
        );
        return PathBuf::from("<unknown>");
    };
    let path = format!("/proc/{pid}/exe");
    match std::fs::read_link(&path) {
        Ok(p) => p,
        Err(e) => {
            log::warn!(
                "auth_request: read_link({path}) failed: {e}; cannot identify peer pid {pid}"
            );
            PathBuf::from("<unknown>")
        }
    }
}

// -----------------------------------------------------------------------------
// Consent helper subprocess
// -----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum ConsentDecision {
    Allow,
    Deny,
    Dismissed,
    PopupFailed,
}

/// Spawn the `super-stt-consent` helper binary, wait up to 60s for the
/// user's decision. The helper writes one of `allow` / `deny` / `dismissed`
/// to stdout and exits.
async fn ask_user_for_consent(app_name: &str, scope: &str, exe_path: &Path) -> ConsentDecision {
    let helper = locate_consent_helper();
    let Some(helper) = helper else {
        log::warn!(
            "super-stt-consent helper not found in PATH or alongside the daemon binary; \
             auth_request will be denied"
        );
        return ConsentDecision::PopupFailed;
    };

    let mut cmd = tokio::process::Command::new(&helper);
    cmd.env("STT_AUTH_APP_NAME", app_name)
        .env("STT_AUTH_SCOPE", scope)
        .env("STT_AUTH_EXE_PATH", exe_path.to_string_lossy().as_ref())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("failed to spawn super-stt-consent: {e}");
            return ConsentDecision::PopupFailed;
        }
    };

    let Some(stdout) = child.stdout.take() else {
        return ConsentDecision::PopupFailed;
    };

    let read_decision = async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut reader = BufReader::new(stdout).lines();
        match reader.next_line().await {
            Ok(Some(line)) => match line.trim() {
                "allow" => ConsentDecision::Allow,
                "deny" => ConsentDecision::Deny,
                _ => ConsentDecision::Dismissed,
            },
            _ => ConsentDecision::Dismissed,
        }
    };

    let result = tokio::time::timeout(Duration::from_mins(1), read_decision).await;
    let _ = child.start_kill();
    let _ = child.wait().await;

    result.unwrap_or(ConsentDecision::Dismissed)
}

/// Find the consent helper.
///
/// **Security model.** The helper is only ever looked for **alongside the
/// daemon binary itself**. We deliberately do NOT fall back to `PATH`
/// because doing so would let any attacker who can prepend a writable
/// directory to the daemon's `PATH` (a classic privilege-escalation
/// vector) substitute their own helper. Forcing co-location bounds the
/// attack surface to "whoever can write to the directory holding the
/// daemon binary" — which is the same threshold required to replace the
/// daemon itself, so we don't make consent any easier to subvert than
/// the daemon's own integrity.
///
/// On top of that, before returning the path:
/// - We `canonicalize()` it, so symlink-swap shenanigans don't help.
/// - We verify the resolved file is owned by the daemon's effective uid
///   (catches "someone dropped a helper they own into the install dir").
/// - We verify it isn't world-writable.
fn locate_consent_helper() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidate = dir.join("super-stt-consent");
    if !candidate.exists() {
        log::warn!(
            "super-stt-consent not found alongside daemon binary at {}; \
             auth_request will be denied with popup_failed",
            candidate.display()
        );
        return None;
    }

    let resolved = match candidate.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            log::warn!(
                "failed to canonicalize consent helper path {}: {e}",
                candidate.display()
            );
            return None;
        }
    };

    if let Err(reason) = verify_helper_metadata(&resolved) {
        log::warn!(
            "consent helper at {} rejected: {reason}",
            resolved.display()
        );
        return None;
    }

    Some(resolved)
}

/// Verify the helper's file metadata is consistent with "trusted binary
/// installed by the user". Returns Err with a static reason on
/// rejection.
#[cfg(unix)]
fn verify_helper_metadata(path: &Path) -> Result<(), &'static str> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).map_err(|_| "cannot stat helper")?;
    // Require the helper to be owned by the same uid the daemon runs
    // as. This catches the case of another local user (or root)
    // dropping a binary into the install dir.
    let our_uid = unsafe { libc::geteuid() };
    if metadata.uid() != our_uid {
        return Err("helper not owned by daemon's effective uid");
    }
    // Reject world-writable helpers — anyone could swap them out.
    if metadata.mode() & 0o002 != 0 {
        return Err("helper is world-writable");
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_helper_metadata(_: &Path) -> Result<(), &'static str> {
    Ok(())
}

// -----------------------------------------------------------------------------
// Endpoint handlers
// -----------------------------------------------------------------------------

async fn dispatch(daemon: &SuperSTTDaemon, request: DaemonRequest) -> DaemonResponse {
    daemon.handle_command(request).await
}

fn build_request(command: &str, data: Option<Value>) -> DaemonRequest {
    DaemonRequest {
        command: command.to_string(),
        audio_data: None,
        sample_rate: None,
        client_id: Some(format!("http-cli-{}", uuid::Uuid::new_v4())),
        event_types: None,
        client_info: None,
        since_timestamp: None,
        limit: None,
        event_type: None,
        data,
        language: None,
        enabled: None,
    }
}

fn json_response(resp: &DaemonResponse) -> (StatusCode, [(&'static str, &'static str); 1], String) {
    let status = if resp.status == "success" {
        StatusCode::OK
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    let body =
        serde_json::to_string(&resp).unwrap_or_else(|_| String::from("{\"status\":\"error\"}"));
    (status, [("content-type", "application/json")], body)
}

async fn ping(State(s): State<AppState>) -> impl IntoResponse {
    let req = build_request("ping", None);
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

async fn status(State(s): State<AppState>) -> impl IntoResponse {
    let req = build_request("status", None);
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

/// `POST /transcribe` — start a daemon-mic recording, streaming named
/// SSE events back as the recording progresses. Same pattern modern
/// LLM streaming APIs (`OpenAI`, Anthropic, Mistral chat, Google Gemini)
/// use: POST with options in the body, response is `text/event-stream`.
///
/// Wire shape:
///
/// ```text
/// event: preview
/// data: {"text":"hello"}
///
/// event: preview
/// data: {"text":"hello world"}
///
/// event: done
/// data: {"transcription":"hello world"}
/// ```
///
/// On daemon-side error a single `event: error\ndata: {"message":"..."}`
/// frame is emitted instead. The stream always ends with one of
/// `done` / `error` and the connection closes. Closing the
/// connection mid-stream cancels the recording (the daemon detects
/// the disconnect on its next write attempt).
/// Emit a single SSE `event: <name>\ndata: <json>\n\n` frame onto the
/// outbound stream. Returns `false` if the receiver is gone (client
/// disconnected). `serde_json::to_string` never embeds raw newlines, so
/// the JSON fits on the single `data:` line by construction.
fn emit_sse_event(
    tx: &tokio::sync::mpsc::UnboundedSender<Result<axum::body::Bytes, std::io::Error>>,
    event: &str,
    data: &serde_json::Value,
) -> bool {
    let mut bytes = format!("event: {event}\ndata: ").into_bytes();
    bytes.extend_from_slice(serde_json::to_string(data).unwrap_or_default().as_bytes());
    bytes.extend_from_slice(b"\n\n");
    tx.send(Ok(axum::body::Bytes::from(bytes))).is_ok()
}

async fn transcribe(
    State(s): State<AppState>,
    body: Option<axum::Json<Value>>,
) -> impl IntoResponse {
    let data = body.map(|axum::Json(v)| v);
    let req = build_request("record", data);

    // mpsc channel that produces SSE byte chunks into the HTTP response body.
    let (line_tx, line_rx) =
        tokio::sync::mpsc::unbounded_channel::<Result<axum::body::Bytes, std::io::Error>>();

    let daemon = Arc::clone(&s.daemon);
    tokio::spawn(async move {
        // Hook into the daemon's preview-text channel so each preview
        // update gets forwarded as an SSE `preview` event.
        let (preview_tx, mut preview_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        *daemon.preview_text.write().await = Some(preview_tx);

        let cmd_fut = daemon.handle_command(req);
        let mut cmd_fut = std::pin::pin!(cmd_fut);
        let mut done = false;
        let mut final_response: Option<DaemonResponse> = None;

        loop {
            tokio::select! {
                // Drive the recording to completion.
                resp = &mut cmd_fut, if !done => {
                    done = true;
                    final_response = Some(resp);
                    *daemon.preview_text.write().await = None;
                }
                // Drain preview text and forward as `preview` events.
                preview = preview_rx.recv() => {
                    match preview {
                        Some(text) => {
                            let payload = serde_json::json!({ "text": text });
                            if !emit_sse_event(&line_tx, "preview", &payload) {
                                // Client disconnected — stop draining.
                                break;
                            }
                        }
                        None if done => break,
                        None => {} // Will get a Some later, keep waiting.
                    }
                }
            }
        }

        if let Some(resp) = final_response {
            if resp.status == "success" {
                let payload = serde_json::json!({
                    "transcription": resp.transcription,
                });
                let _ = emit_sse_event(&line_tx, "done", &payload);
            } else {
                let payload = serde_json::json!({
                    "message": resp.message,
                });
                let _ = emit_sse_event(&line_tx, "error", &payload);
            }
        }
    });

    let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(line_rx);
    let body = axum::body::Body::from_stream(stream);

    axum::response::Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-store")
        .header("x-accel-buffering", "no") // disable proxy buffering, just in case
        .body(body)
        .unwrap_or_else(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [("content-type", "application/json")],
                String::from("{\"status\":\"error\"}"),
            )
                .into_response()
        })
}

async fn transcribe_stop(State(s): State<AppState>) -> impl IntoResponse {
    let data = serde_json::json!({
        "write_mode": false,
        "stop_mode": "manual-only",
    });
    let req = build_request("record", Some(data));
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

// ---------- /events (widget SSE) -------------------------------------------

/// Topics a `widget`-scoped token is allowed to subscribe to. Settings
/// tokens skip this check (god-mode). Per `docs/protocol/widget.md`.
const WIDGET_TOPICS: &[&str] = &[
    "recording_started",
    "recording_stopped",
    "recording_state",
    "audio_samples",
    "frequency_bands",
    "partial_stt",
    "final_stt",
];

#[derive(serde::Deserialize)]
struct EventsQuery {
    /// Comma-separated topic names. Empty / missing → 400 `invalid_topic`.
    topics: Option<String>,
}

fn invalid_topic(reason: &str) -> Response {
    let body = serde_json::json!({
        "status":  "error",
        "message": "invalid_topic",
        "data":    { "reason": reason },
    });
    (
        StatusCode::BAD_REQUEST,
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// `GET /events?topics=...` — widget SSE subscription.
///
/// The handler runs forever (until the client disconnects, the daemon
/// shuts down, or `exe_changed` triggers a revoked event). Each
/// requested topic gets a per-connection `broadcast::Receiver` which
/// runs in its own forwarder task — so all subscribers receive events
/// independently and a slow widget never starves a fast one (and vice
/// versa).
///
/// The forwarder tasks share a `CancellationToken` with the keepalive
/// + exe-watch task, so any of `client disconnect / exe_changed /
/// shutdown` cleanly tears the whole subscription down.
async fn events(
    State(s): State<AppState>,
    Query(q): Query<EventsQuery>,
    ctx: Option<axum::Extension<AuthContext>>,
    peer: Option<axum::Extension<PeerInfo>>,
) -> Response {
    let requested = match parse_events_topics(&q) {
        Ok(t) => t,
        Err(reason) => return invalid_topic(&reason),
    };

    // Auth context — should always be present after middleware ran, but
    // we degrade gracefully if it isn't (treat as missing session).
    let Some(axum::Extension(ctx)) = ctx else {
        return invalid_session("unknown");
    };

    // Widget tokens are restricted to the audio/recording/STT topic
    // set. Settings tokens skip this check (god-mode).
    if ctx.meta.scope == "widget"
        && requested
            .iter()
            .any(|t| !WIDGET_TOPICS.contains(&t.as_str()))
    {
        return scope_denied();
    }

    // mpsc that serializes all SSE writes (broadcast forwarders +
    // keepalive + revocation).
    let (sse_tx, sse_rx) =
        tokio::sync::mpsc::unbounded_channel::<Result<axum::body::Bytes, std::io::Error>>();

    // Initial `subscribed` event with a fresh subscriber id and the
    // confirmed topic list — this is what tells the client its
    // subscription was accepted.
    let topic_names: Vec<&'static str> = requested.iter().map(|t| t.as_str()).collect();
    let _ = emit_sse_event(
        &sse_tx,
        "subscribed",
        &serde_json::json!({
            "client_id": uuid::Uuid::new_v4().to_string(),
            "subscribed_to": topic_names,
        }),
    );

    // Subscribe + spawn forwarders. Cancellation propagates to the
    // keepalive/exe-watch task through the shared token.
    let cancel = tokio_util::sync::CancellationToken::new();
    for topic in &requested {
        let rx = s.daemon.events.subscribe(*topic);
        spawn_topic_forwarder(rx, sse_tx.clone(), cancel.clone());
    }
    spawn_events_keepalive_and_exe_watch(
        sse_tx.clone(),
        cancel,
        peer.and_then(|p| p.0.pid),
        s.tokens.clone(),
        ctx.token,
        ctx.meta.exe_path,
    );

    // The handler's own `sse_tx` clone is dropped here. The forwarders
    // and the timer task own the remaining clones; once they all
    // finish, the mpsc receiver yields None and the response body ends.
    drop(sse_tx);

    let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(sse_rx);
    let body = axum::body::Body::from_stream(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-store")
        .header("x-accel-buffering", "no")
        .body(body)
        .unwrap_or_else(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [("content-type", "application/json")],
                String::from("{\"status\":\"error\"}"),
            )
                .into_response()
        })
}

/// Parse the `?topics=` query string into a deduplicated `Vec<Topic>`.
/// Returns the raw bad-topic name (or `"missing_topics"` for missing /
/// empty queries) on the `Err` arm so the caller can produce the
/// matching `400 invalid_topic` response. Boxing the response would be
/// fine but using a small string keeps the error type cheap to clone /
/// move.
fn parse_events_topics(q: &EventsQuery) -> Result<Vec<crate::daemon::events::Topic>, String> {
    use crate::daemon::events::Topic;

    let csv = match q.topics.as_deref() {
        Some(s) if !s.is_empty() => s,
        _ => return Err("missing_topics".to_string()),
    };
    let mut requested: Vec<Topic> = Vec::new();
    for raw in csv.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        match Topic::from_wire(raw) {
            Some(t) if !requested.contains(&t) => requested.push(t),
            Some(_) => {} // duplicate
            None => return Err(raw.to_string()),
        }
    }
    if requested.is_empty() {
        return Err("missing_topics".to_string());
    }
    Ok(requested)
}

/// Spawn a per-topic forwarder. Reads from the broadcast receiver and
/// writes each event as an SSE frame. Exits on cancel, on a closed
/// channel, or when the SSE response body has been dropped.
fn spawn_topic_forwarder(
    mut rx: crate::daemon::events::AnyReceiver,
    tx: tokio::sync::mpsc::UnboundedSender<Result<axum::body::Bytes, std::io::Error>>,
    cancel: tokio_util::sync::CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                res = rx.recv_json() => {
                    match res {
                        Ok((name, payload)) => {
                            if !emit_sse_event(&tx, name, &payload) {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            log::warn!("widget SSE lagged: dropped {n} events");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    });
}

/// Spawn the timer task that drives keep-alive comments and the
/// periodic exe-path check. On `exe_changed` the task emits a
/// `revoked` event, calls `TokenStore::revoke`, and triggers the
/// shared `cancel` token to tear down the rest of the subscription.
fn spawn_events_keepalive_and_exe_watch(
    tx: tokio::sync::mpsc::UnboundedSender<Result<axum::body::Bytes, std::io::Error>>,
    cancel: tokio_util::sync::CancellationToken,
    peer_pid: Option<u32>,
    tokens: TokenStore,
    token_str: String,
    stored_exe: PathBuf,
) {
    use tokio::time::{Duration, MissedTickBehavior, interval};

    tokio::spawn(async move {
        // Both timers are 30 s (cheap), aligned by `MissedTickBehavior::Skip`
        // so a temporarily-blocked task doesn't accumulate stale ticks.
        let mut keepalive = interval(Duration::from_secs(30));
        keepalive.set_missed_tick_behavior(MissedTickBehavior::Skip);
        keepalive.tick().await; // immediate first tick — discard
        let mut exe_watch = interval(Duration::from_secs(30));
        exe_watch.set_missed_tick_behavior(MissedTickBehavior::Skip);
        exe_watch.tick().await;

        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                _ = keepalive.tick() => {
                    if tx
                        .send(Ok(axum::body::Bytes::from_static(b": keepalive\n\n")))
                        .is_err()
                    {
                        cancel.cancel();
                        break;
                    }
                }
                _ = exe_watch.tick() => {
                    let Some(pid) = peer_pid else { continue; };
                    let current = std::fs::read_link(format!("/proc/{pid}/exe")).ok();
                    if current.as_ref().is_some_and(|c| *c == stored_exe) {
                        continue;
                    }
                    log::info!(
                        "widget exe_changed on pid {pid}: stored={} current={:?}; revoking session",
                        stored_exe.display(),
                        current,
                    );
                    let _ = emit_sse_event(
                        &tx,
                        "revoked",
                        &serde_json::json!({ "reason": "exe_changed" }),
                    );
                    tokens.revoke(&token_str);
                    cancel.cancel();
                    break;
                }
            }
        }
    });
}

// `Read` import suppresses an unused-import warning if/when consent helper
// stdout reading switches forms.
#[allow(dead_code)]
fn _bytes_read(_: &mut dyn Read) {}

// =============================================================================
// Settings-scope endpoints
// =============================================================================
//
// All of these dispatch into the existing legacy `Command::*` handlers via
// `daemon.handle_command(...)`. Business logic stays in the legacy handlers;
// the HTTP layer is purely a translator.

// ---------- /active_model ----------

#[derive(Deserialize)]
struct SetActiveModelBody {
    model: String,
    provider: String,
    #[serde(default)]
    source: Option<String>,
}

async fn set_active_model(
    State(s): State<AppState>,
    axum::Json(body): axum::Json<SetActiveModelBody>,
) -> impl IntoResponse {
    let mut data = serde_json::json!({
        "model":    body.model,
        "provider": body.provider,
    });
    if let Some(source) = body.source {
        data["source"] = serde_json::Value::String(source);
    }
    let req = build_request("set_model", Some(data));
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

async fn get_active_model(State(s): State<AppState>) -> impl IntoResponse {
    // Compose the legacy `get_model` + `get_device` + `get_download_status`
    // results into the doc-spec `{ active_model: { current, switch } }` shape.
    let model_resp = dispatch(&s.daemon, build_request("get_model", None)).await;
    let device_resp = dispatch(&s.daemon, build_request("get_device", None)).await;
    let download_resp = dispatch(&s.daemon, build_request("get_download_status", None)).await;

    let switch_payload = download_resp.download_progress.map(|p| {
        serde_json::json!({
            "phase":            p.status,
            "target":           { "model": p.model_name },
            "started_at":       p.started_at,
            "download": {
                "current_file":     p.current_file,
                "file_index":       p.file_index,
                "total_files":      p.total_files,
                "bytes_downloaded": p.bytes_downloaded,
                "total_bytes":      p.total_bytes,
                "percentage":       p.percentage,
                "eta_seconds":      p.eta_seconds,
            },
        })
    });

    let body = serde_json::json!({
        "status": "success",
        "active_model": {
            "current": {
                "model":    model_resp.current_model,
                "provider": model_resp.current_provider,
                "source":   model_resp.current_source,
                "loaded":   model_resp.model_loaded.unwrap_or(false),
                "device":   device_resp.device.unwrap_or_else(|| "unknown".to_string()),
            },
            "switch": switch_payload,
        }
    });
    (
        StatusCode::OK,
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

async fn cancel_set_active_model(State(s): State<AppState>) -> impl IntoResponse {
    let req = build_request("cancel_download", None);
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

async fn list_models(State(s): State<AppState>) -> impl IntoResponse {
    let req = build_request("list_models", None);
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

// ---------- /active_device ----------

#[derive(Deserialize)]
struct SetActiveDeviceBody {
    device: String,
}

async fn set_active_device(
    State(s): State<AppState>,
    axum::Json(body): axum::Json<SetActiveDeviceBody>,
) -> impl IntoResponse {
    let req = build_request(
        "set_device",
        Some(serde_json::json!({ "device": body.device })),
    );
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

async fn get_active_device(State(s): State<AppState>) -> impl IntoResponse {
    let req = build_request("get_device", None);
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

// ---------- /audio_theme ----------

#[derive(Deserialize)]
struct SetAudioThemeBody {
    theme: String,
}

async fn set_audio_theme(
    State(s): State<AppState>,
    axum::Json(body): axum::Json<SetAudioThemeBody>,
) -> impl IntoResponse {
    let req = build_request(
        "set_audio_theme",
        Some(serde_json::json!({ "theme": body.theme })),
    );
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

async fn get_audio_theme(State(s): State<AppState>) -> impl IntoResponse {
    let req = build_request("get_audio_theme", None);
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

async fn test_audio_theme(State(s): State<AppState>) -> impl IntoResponse {
    let req = build_request("test_audio_theme", None);
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

async fn list_audio_themes(State(s): State<AppState>) -> impl IntoResponse {
    let req = build_request("list_audio_themes", None);
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

// ---------- /volume ----------

#[derive(Deserialize)]
struct SetVolumeBody {
    volume: u8,
}

async fn set_volume(
    State(s): State<AppState>,
    axum::Json(body): axum::Json<SetVolumeBody>,
) -> impl IntoResponse {
    let req = build_request(
        "set_volume",
        Some(serde_json::json!({ "volume": body.volume })),
    );
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

async fn get_volume(State(s): State<AppState>) -> impl IntoResponse {
    let req = build_request("get_volume", None);
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

// ---------- /recording_stop_mode ----------

#[derive(Deserialize)]
struct SetRecordingStopModeBody {
    mode: String,
}

async fn set_recording_stop_mode(
    State(s): State<AppState>,
    axum::Json(body): axum::Json<SetRecordingStopModeBody>,
) -> impl IntoResponse {
    let req = build_request(
        "set_recording_stop_mode",
        Some(serde_json::json!({ "mode": body.mode })),
    );
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

async fn get_recording_stop_mode(State(s): State<AppState>) -> impl IntoResponse {
    let req = build_request("get_recording_stop_mode", None);
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

// ---------- /write_method ----------

#[derive(Deserialize)]
struct SetWriteMethodBody {
    method: String,
}

async fn set_write_method(
    State(s): State<AppState>,
    axum::Json(body): axum::Json<SetWriteMethodBody>,
) -> impl IntoResponse {
    let req = build_request(
        "set_write_method",
        Some(serde_json::json!({ "method": body.method })),
    );
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

async fn get_write_method(State(s): State<AppState>) -> impl IntoResponse {
    let req = build_request("get_write_method", None);
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

// ---------- /preview_typing ----------

#[derive(Deserialize)]
struct PreviewTypingBody {
    enabled: bool,
}

async fn set_preview_typing(
    State(s): State<AppState>,
    axum::Json(body): axum::Json<PreviewTypingBody>,
) -> impl IntoResponse {
    // The legacy command takes `enabled` at the top level of
    // DaemonRequest, not inside `data`.
    let mut req = build_request("set_preview_typing", None);
    req.enabled = Some(body.enabled);
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

async fn get_preview_typing(State(s): State<AppState>) -> impl IntoResponse {
    let req = build_request("get_preview_typing", None);
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

// ---------- /allow_online_models ----------

#[derive(Deserialize)]
struct AllowOnlineModelsBody {
    enabled: bool,
}

async fn set_allow_online_models(
    State(s): State<AppState>,
    axum::Json(body): axum::Json<AllowOnlineModelsBody>,
) -> impl IntoResponse {
    let mut req = build_request("set_allow_online_models", None);
    req.enabled = Some(body.enabled);
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

async fn get_allow_online_models(State(s): State<AppState>) -> impl IntoResponse {
    let req = build_request("get_allow_online_models", None);
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

// ---------- /custom_models_dir ----------

#[derive(Deserialize)]
struct CustomModelsDirBody {
    path: Option<String>,
}

async fn set_custom_models_dir(
    State(s): State<AppState>,
    axum::Json(body): axum::Json<CustomModelsDirBody>,
) -> impl IntoResponse {
    let req = build_request(
        "set_custom_models_dir",
        Some(serde_json::json!({ "path": body.path })),
    );
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

async fn get_custom_models_dir(State(s): State<AppState>) -> impl IntoResponse {
    let req = build_request("get_custom_models_dir", None);
    let resp = dispatch(&s.daemon, req).await;
    json_response(&resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_meta(app: &str, scope: &str, exe: &str, expires_at: DateTime<Utc>) -> TokenMeta {
        TokenMeta {
            app_name: app.to_string(),
            scope: scope.to_string(),
            exe_path: PathBuf::from(exe),
            issued_at: Utc::now(),
            expires_at,
        }
    }

    /// `keyring_writes_in_cooldown` must report true for any failure
    /// timestamp inside the cooldown window and false outside it.
    /// Drives the `flush_locked` short-circuit that prevents
    /// re-prompting the user every few seconds when the keyring is
    /// locked.
    #[test]
    fn keyring_failure_cooldown_window_behavior() {
        clear_keyring_failure_flag();
        assert!(!keyring_writes_in_cooldown(), "no failure recorded");

        mark_keyring_failure();
        assert!(
            keyring_writes_in_cooldown(),
            "freshly marked failure must suppress writes"
        );

        // Push the timestamp far enough into the past to exit the
        // cooldown window. We hold the lock briefly to backdate.
        {
            let mut guard = KEYRING_LAST_FAILURE.lock().unwrap();
            let before = std::time::Instant::now()
                .checked_sub(KEYRING_FAILURE_COOLDOWN + std::time::Duration::from_secs(1))
                .expect("backdated instant");
            *guard = Some(before);
        }
        assert!(
            !keyring_writes_in_cooldown(),
            "failure older than cooldown window must allow writes again"
        );

        clear_keyring_failure_flag();
        assert!(
            !keyring_writes_in_cooldown(),
            "explicit clear must reset the flag"
        );
    }

    #[test]
    fn sessions_file_round_trips_through_json() {
        let mut sessions = HashMap::new();
        sessions.insert(
            "tok-a".to_string(),
            make_meta(
                "App A",
                "settings",
                "/usr/bin/app-a",
                Utc::now() + ChronoDuration::days(30),
            ),
        );
        sessions.insert(
            "tok-b".to_string(),
            make_meta(
                "App B",
                "client",
                "/usr/bin/app-b",
                Utc::now() + ChronoDuration::days(7),
            ),
        );
        let payload = SessionsFile {
            version: SESSIONS_SCHEMA_VERSION,
            sessions,
        };
        let json = serde_json::to_string(&payload).expect("serialize");
        let back: SessionsFile = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.version, SESSIONS_SCHEMA_VERSION);
        assert_eq!(back.sessions.len(), 2);
        assert_eq!(back.sessions["tok-a"].app_name, "App A");
        assert_eq!(back.sessions["tok-b"].scope, "client");
    }

    /// `auth_request` always runs the consent flow now (no
    /// identity-only reuse-scan), so a token can only be retrieved
    /// from the store by presenting it via the bearer header to a
    /// protected endpoint. `validate` is the path that exercises
    /// that. We just verify it returns the right metadata for a
    /// known-good token and rejects unknown ones.
    #[test]
    fn validate_returns_meta_for_known_token_and_unknown_for_others() {
        let store = TokenStore::default();
        store.inner.lock().unwrap().insert(
            "live".to_string(),
            make_meta(
                "App",
                "settings",
                "/usr/bin/app",
                Utc::now() + ChronoDuration::hours(1),
            ),
        );
        let meta = store.validate("live").expect("known token validates");
        assert_eq!(meta.app_name, "App");
        assert_eq!(meta.scope, "settings");
        assert_eq!(meta.exe_path, PathBuf::from("/usr/bin/app"));

        let err = store.validate("nope").expect_err("unknown token rejected");
        assert_eq!(err, "unknown");
    }

    #[test]
    fn validate_removes_expired_token_in_place() {
        let store = TokenStore::default();
        store.inner.lock().unwrap().insert(
            "stale".to_string(),
            make_meta(
                "App",
                "client",
                "/usr/bin/app",
                Utc::now() - ChronoDuration::seconds(1),
            ),
        );
        // We can't actually invoke flush_locked here (it would try to
        // touch the real keyring), but we can exercise the in-memory
        // half of `validate` by calling it and confirming the entry is
        // gone afterwards. flush_locked logs-and-swallows on failure,
        // so the test stays hermetic.
        let err = store.validate("stale").expect_err("should be expired");
        assert_eq!(err, "expired");
        assert!(store.inner.lock().unwrap().get("stale").is_none());
    }

    fn deny_key(app: &str, exe: &str, scope: &str) -> ConsentKey {
        (app.to_string(), PathBuf::from(exe), scope.to_string())
    }

    /// Sticky deny: once `insert(key)` runs, `contains(key)` must keep
    /// returning true for the rest of the daemon's lifetime. This is
    /// the load-bearing invariant of the deny cache — break it and
    /// the daemon will start re-prompting users who already said no.
    #[test]
    fn deny_cache_insert_then_contains_returns_true() {
        let cache = DenyCache::default();
        let key = deny_key("Super STT App", "/usr/bin/super-stt-app", "settings");
        assert!(
            !cache.contains(&key),
            "fresh cache must not report any key as present"
        );

        cache.insert(key.clone());
        assert!(
            cache.contains(&key),
            "after insert, the same key must hit the cache"
        );
    }

    /// Different `(app_name, exe_path, scope)` triples must be
    /// distinguished. Otherwise an unrelated app's denial would
    /// poison every other app's consent flow.
    #[test]
    fn deny_cache_distinguishes_keys_by_each_component() {
        let cache = DenyCache::default();
        let app_a = deny_key("App A", "/usr/bin/a", "widget");
        let app_b_same_path = deny_key("App B", "/usr/bin/a", "widget");
        let app_a_other_path = deny_key("App A", "/usr/bin/a-renamed", "widget");
        let app_a_other_scope = deny_key("App A", "/usr/bin/a", "settings");

        cache.insert(app_a.clone());
        assert!(cache.contains(&app_a));
        assert!(
            !cache.contains(&app_b_same_path),
            "different app_name must not collide"
        );
        assert!(
            !cache.contains(&app_a_other_path),
            "different exe_path must not collide"
        );
        assert!(
            !cache.contains(&app_a_other_scope),
            "different scope must not collide"
        );
    }

    /// `insert` is a set add, not a list append — calling it twice
    /// with the same key is a no-op. We're not strictly testing
    /// HashSet semantics (that's stdlib's job); we're documenting
    /// the contract DenyCache exposes so a future refactor can't
    /// accidentally swap it for a duplicating store.
    #[test]
    fn deny_cache_insert_is_idempotent() {
        let cache = DenyCache::default();
        let key = deny_key("App", "/usr/bin/app", "client");
        cache.insert(key.clone());
        cache.insert(key.clone());
        cache.insert(key.clone());
        // Internal length check: ensures we didn't grow a duplicate
        // entry that would leak memory across many denies.
        assert_eq!(cache.inner.lock().unwrap().len(), 1);
        assert!(cache.contains(&key));
    }

    /// The cache is purely in-memory and lives on the daemon's
    /// AppState. Two independent `DenyCache::default()` instances
    /// must not share state — that's what gives us the "daemon
    /// restart clears the deny cache" guarantee documented in
    /// auth.md.
    #[test]
    fn deny_cache_instances_do_not_share_state() {
        let a = DenyCache::default();
        let b = DenyCache::default();
        let key = deny_key("App", "/usr/bin/app", "widget");

        a.insert(key.clone());
        assert!(a.contains(&key));
        assert!(
            !b.contains(&key),
            "a fresh DenyCache (e.g. after daemon restart) must start empty"
        );
    }
}
