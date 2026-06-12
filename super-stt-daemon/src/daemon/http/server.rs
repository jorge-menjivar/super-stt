// SPDX-License-Identifier: GPL-3.0-only
//! HTTP server for the daemon protocol.
//!
//! Binds `$XDG_RUNTIME_DIR/stt/super-stt-http.sock` (override via
//! `SUPER_STT_HTTP_SOCKET`). All endpoints are served under the `/v1`
//! URL prefix — route definitions in `build_router` are written as
//! bare paths (`/ping`, `/auth/request`, …) and the prefix is applied
//! via a single `Router::nest("/v1", …)` at the bottom of the file.
//!
//! v1 endpoint set (wire paths):
//!
//! - `POST /v1/auth/request`     — interactive consent → mints a session token
//! - `GET  /v1/auth/status`      — probe token validity (no consent UI)
//! - `GET  /v1/ping`             — liveness (any authenticated token)
//! - `GET  /v1/status`           — current model + device (`status` scope)
//! - `POST /v1/transcribe`       — start a daemon-mic recording
//! - `POST /v1/transcribe/stop`  — stop an in-flight daemon-mic recording
//! - `GET  /v1/events?topics=…`  — Server-Sent Events stream (per-topic scope)
//! - … plus the settings configuration surface (see [`build_router`])
//!
//! Authentication:
//! - The daemon uses `SO_PEERCRED` on each connection to get the peer PID
//!   and resolves `/proc/<pid>/exe`. That path is shown in the consent
//!   popup so the user knows which binary is asking.
//! - On Allow, the daemon mints a 32-byte hex session token and stores it
//!   keyed in an in-memory `TokenStore`. The token has a 30-day expiry.
//! - Every endpoint other than `/v1/auth/request` requires
//!   `Authorization: Bearer <token>`. Missing/invalid → 401 with
//!   `{ status: "error", message: "invalid_session", data: { reason } }`.
//! - The popup is the `super-stt-consent` helper binary, spawned as a
//!   subprocess. It writes "allow" / "deny" / "dismissed" to stdout.
//! - Set `SUPER_STT_AUTO_APPROVE=1` in the daemon environment to skip
//!   the popup entirely (intended for tests / CI).

#[cfg(test)]
use crate::daemon::http::internal::auth::consent::ConsentKey;
#[cfg(test)]
use crate::daemon::http::internal::auth::middleware::DenyCache;
#[cfg(test)]
use crate::daemon::http::internal::auth::tokens::TokenMeta;
#[cfg(test)]
use crate::daemon::http::internal::auth::tokens::TokenStore;
#[cfg(test)]
use crate::daemon::http::internal::auth::tokens::{
    KEYRING_FAILURE_COOLDOWN, KEYRING_LAST_FAILURE, SESSIONS_SCHEMA_VERSION, SessionsFile,
    clear_keyring_failure_flag, keyring_writes_in_cooldown, mark_keyring_failure,
};
use crate::daemon::http::state::{AppState, PeerInfo};
use crate::daemon::types::SuperSTTDaemon;
use anyhow::{Context, Result};
use axum::http::StatusCode;
#[cfg(test)]
use chrono::Duration as ChronoDuration;
#[cfg(test)]
use chrono::{DateTime, Utc};
use log::{info, warn};
#[cfg(test)]
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio::sync::broadcast;

/// Env var that, when set to "1", bypasses the consent popup entirely
/// and auto-approves every `auth_request`. Intended for tests / CI only.
pub const AUTO_APPROVE_ENV: &str = "SUPER_STT_AUTO_APPROVE";

/// Create the parent directory, remove any stale socket file, bind the
/// Unix listener, and set socket permissions.
///
/// # Errors
/// Returns an error if directory creation, stale-file removal, socket
/// bind, or permission setting fails.
async fn bind_listener(socket_path: &std::path::Path) -> Result<UnixListener> {
    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("Failed to create http socket directory")?;
    }
    if socket_path.exists() {
        tokio::fs::remove_file(socket_path)
            .await
            .context("Failed to remove existing http socket file")?;
    }

    let listener = UnixListener::bind(socket_path).context("Failed to bind http Unix socket")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if cfg!(debug_assertions) { 0o666 } else { 0o660 };
        let perms = std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(socket_path, perms)
            .context("Failed to set http socket permissions")?;
    }

    Ok(listener)
}

/// Spawn the HTTP server on the dedicated Unix socket. Returns once the
/// listener is bound; the actual accept loop runs in a background task.
///
/// Returns the [`JoinHandle`] of the spawned accept-loop task so the
/// caller can supervise it: if the task ends before `shutdown_tx`
/// fires (panic, fatal `accept()` error, etc.), the caller should
/// treat the daemon as unreachable and exit. See
/// [`daemon_main::run`](crate::daemon_main::run) for the supervision
/// `tokio::select!`.
///
/// # Errors
/// Returns an error if the socket can't be created or bound.
pub async fn start_http_server(
    daemon: Arc<SuperSTTDaemon>,
    socket_path: PathBuf,
    shutdown_tx: broadcast::Sender<()>,
) -> Result<tokio::task::JoinHandle<()>> {
    // Build the application state first. This loads the persisted session
    // store from the system keyring; if the keyring is unavailable the
    // daemon refuses to start (see `TokenStore::load_persisted`). Doing it
    // before binding the listener guarantees we never leave a
    // listening-but-unserviced socket behind on a keyring failure.
    //
    // That keyring read is a *blocking* secret-service call: on a locked
    // keyring it waits on the D-Bus unlock prompt for as long as the user
    // takes. Run it on the blocking pool and race it against the shutdown
    // signal so the wait stays interruptible — otherwise a Ctrl+C during
    // the unlock wait is swallowed (the supervision `select!` is only
    // reached once startup finishes) and the daemon can't be stopped.
    let state = {
        let daemon = Arc::clone(&daemon);
        let mut shutdown_rx = shutdown_tx.subscribe();
        let load = tokio::task::spawn_blocking(move || AppState::new(daemon));
        tokio::select! {
            biased;
            _ = shutdown_rx.recv() => {
                // The blocking load is still parked on the keyring prompt;
                // a dropped runtime would join (and hang on) it, so exit
                // the process directly. Nothing is bound or in flight yet.
                info!("Shutdown requested during session-store load; exiting");
                std::process::exit(130);
            }
            res = load => res.context("session-store load task panicked")??,
        }
    };
    let app = crate::daemon::http::v1::router(state);

    let listener = bind_listener(&socket_path).await?;

    info!(
        "HTTP daemon listening on socket: {} (side-by-side with legacy listener)",
        socket_path.display()
    );

    let cleanup_path = socket_path.clone();
    let handle = tokio::spawn(async move {
        let mut shutdown_rx = shutdown_tx.subscribe();
        let resource_manager = Arc::clone(&daemon.resource_manager);
        let server_loop = async {
            loop {
                let (stream, _addr) = match listener.accept().await {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("http accept failed: {e}");
                        continue;
                    }
                };
                let peer_cred = stream.peer_cred().ok();
                let resolved_process_id = peer_cred
                    .as_ref()
                    .and_then(tokio::net::unix::UCred::pid)
                    .map(|p| u32::try_from(p).unwrap_or(0));
                let resolved_user_id = peer_cred.as_ref().map(tokio::net::unix::UCred::uid);
                let peer = PeerInfo {
                    pid: resolved_process_id,
                    uid: resolved_user_id,
                };

                // 503 connection_rejected if the per-client cap is
                // hit. We have to write the response by hand here
                // since axum isn't in the picture yet — we never
                // hand the stream to serve_connection.
                //
                // Registration is idempotent per client_id (uid:pid):
                // each call upserts the entry, the cap-check passes
                // when the entry already exists, and we deliberately
                // do NOT call `unregister_connection` on conn-close.
                // The same client_id may have multiple concurrent
                // connections (e.g., /v1/events open while /v1/ping
                // fires), and an eager unregister would brick the
                // sibling's rate-limit lookup. Stale entries are
                // pruned by `ResourceManager::cleanup_task` after
                // the configured idle timeout.
                let client_id = peer.client_id();
                if let Err(e) = resource_manager
                    .register_connection(client_id.clone(), None)
                    .await
                {
                    warn!(
                        "connection rejected for {client_id}: {e}; sending 503 connection_rejected"
                    );
                    let _ = write_oneshot_response(
                        stream,
                        StatusCode::SERVICE_UNAVAILABLE,
                        "connection_rejected",
                    )
                    .await;
                    continue;
                }

                let app_for_conn = app.clone().layer(axum::Extension(peer.clone()));
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
            biased;
            _ = shutdown_rx.recv() => {
                info!("HTTP server shutting down");
            }
            _ = server_loop => {}
        }
        if cleanup_path.exists() {
            let _ = tokio::fs::remove_file(&cleanup_path).await;
        }
    });

    Ok(handle)
}

/// Write a single short HTTP/1.1 error response directly to a Unix
/// stream, bypassing axum/hyper. Used in the accept loop when we
/// need to reject a connection (e.g., `503 connection_rejected`)
/// before handing the stream to `serve_connection`.
async fn write_oneshot_response(
    mut stream: tokio::net::UnixStream,
    status: StatusCode,
    message: &str,
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let body = format!(r#"{{"status":"error","message":"{message}"}}"#);
    let reason = status.canonical_reason().unwrap_or("Unknown");
    let raw = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status.as_u16(),
        reason,
        body.len(),
        body,
    );
    stream.write_all(raw.as_bytes()).await?;
    stream.shutdown().await
}

// (auth/request, auth/status, ping, status, transcribe, events, settings moved to v1/; see use re-imports above)

#[cfg(test)]
mod tests {
    use super::*;

    fn make_meta(app: &str, scope: &str, exe: &str, expires_at: DateTime<Utc>) -> TokenMeta {
        TokenMeta {
            app_name: app.to_string(),
            scopes: vec![scope.to_string()],
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
                "transcribe",
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
        assert_eq!(
            back.sessions["tok-b"].scopes,
            vec!["transcribe".to_string()]
        );
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
        assert_eq!(meta.scopes, vec!["settings".to_string()]);
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
                "transcribe",
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

    fn deny_key(_app: &str, exe: &str, scope: &str) -> ConsentKey {
        // ConsentKey dropped `app_name` from the tuple: app_name is
        // client-controlled and untrusted, so the deny key is now
        // `(exe_path, scopes)` only. We keep the `_app` arg in the test
        // helper signature so we don't have to rewrite every call site.
        (PathBuf::from(exe), vec![scope.to_string()])
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

    /// Different `(exe_path, scope)` pairs must be distinguished —
    /// otherwise an unrelated binary's denial would poison every
    /// other binary's consent flow. `app_name` is intentionally NOT
    /// part of the key: it's client-controlled, so a misbehaving
    /// caller could otherwise bypass a deny by rotating its declared
    /// app name. Two requests from the same binary in the same scope
    /// SHOULD collide regardless of app_name — that's the bug fix.
    #[test]
    fn deny_cache_distinguishes_keys_by_each_component() {
        let cache = DenyCache::default();
        let app_a = deny_key("App A", "/usr/bin/a", "recording_events");
        let same_path_renamed = deny_key("Renamed App", "/usr/bin/a", "recording_events");
        let other_path = deny_key("App A", "/usr/bin/a-renamed", "recording_events");
        let other_scope = deny_key("App A", "/usr/bin/a", "settings");

        cache.insert(app_a.clone());
        assert!(cache.contains(&app_a));
        assert!(
            cache.contains(&same_path_renamed),
            "same exe_path + scope must collide regardless of declared app_name \
             (denial sticks to the binary, not the self-reported name)"
        );
        assert!(
            !cache.contains(&other_path),
            "different exe_path must not collide"
        );
        assert!(
            !cache.contains(&other_scope),
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
        let key = deny_key("App", "/usr/bin/app", "transcribe");
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
        let key = deny_key("App", "/usr/bin/app", "recording_events");

        a.insert(key.clone());
        assert!(a.contains(&key));
        assert!(
            !b.contains(&key),
            "a fresh DenyCache (e.g. after daemon restart) must start empty"
        );
    }
}
