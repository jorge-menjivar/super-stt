// SPDX-License-Identifier: GPL-3.0-only
//! HTTP server for the daemon protocol.
//!
//! The listeners and the auth layer are `super-engine-daemon`'s, shared with
//! Super TTS; this names Super STT's router, keyring and consent dialog to
//! them.
//!
//! Binds `$XDG_RUNTIME_DIR/stt/super-stt-http.sock` (override via
//! `SUPER_STT_HTTP_SOCKET`). All endpoints are served under the `/v1`
//! URL prefix — route definitions in `build_router` are written as
//! bare paths (`/ping`, `/auth/request`, …) and the prefix is applied
//! via a single `Router::nest("/v1", …)` at the bottom of the file.
//!
//! v1 endpoint set (wire paths):
//!
//! - `POST /v1/auth/request`              — interactive consent → mints a session token
//! - `GET  /v1/auth/status`               — probe token validity (no consent UI)
//! - `GET  /v1/ping`                      — liveness (any authenticated token)
//! - `GET  /v1/status`                    — current model + device (`status` scope)
//! - `POST /v1/transcribe`                — start a daemon-mic recording
//! - `POST /v1/transcribe/stop`           — stop an in-flight daemon-mic recording
//! - `GET  /v1/events?topics=…`           — Server-Sent Events stream (per-topic scope)
//! - `GET  /v1/update`                    — last self-update check result (`settings` scope)
//! - `POST /v1/update/check`              — force an immediate self-update check (`settings` scope)
//! - `GET/POST /v1/update_check_enabled`  — periodic self-update check on/off (`settings` scope)
//! - `GET/POST /v1/update_beta_optin`     — self-update prerelease opt-in (`settings` scope)
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

use crate::daemon::http::state::AppState;
use crate::daemon::types::SuperSTTDaemon;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use super_engine_daemon::auth::consent::ConsentDialog;
use super_engine_daemon::auth::{Auth, AuthConfig};
use super_engine_daemon::http::server::{Listeners, serve};
use super_stt_shared::SUPER_STT;
use tokio::sync::broadcast;

/// Env var that, when set to "1", bypasses the consent popup entirely
/// and auto-approves every `auth_request`. Intended for tests / CI only.
pub const AUTO_APPROVE_ENV: &str = "SUPER_STT_AUTO_APPROVE";

/// Spawn the HTTP server on the dedicated Unix socket, and on the loopback
/// TCP port when `[http.tcp]` turns it on. Returns once the listeners are
/// bound; the actual accept loop runs in a background task.
///
/// Returns the [`JoinHandle`](tokio::task::JoinHandle) of the spawned
/// accept-loop task so the caller can supervise it: if the task ends before
/// `shutdown_tx` fires (panic, fatal `accept()` error, etc.), the caller should
/// treat the daemon as unreachable and exit. See
/// [`daemon_main::run`](crate::daemon_main::run) for the supervision
/// `tokio::select!`.
///
/// # Errors
/// Returns an error if the system keyring is unavailable, or the socket
/// can't be created or bound.
pub async fn start_http_server(
    daemon: Arc<SuperSTTDaemon>,
    socket_path: PathBuf,
    shutdown_tx: broadcast::Sender<()>,
) -> Result<tokio::task::JoinHandle<()>> {
    let tcp = daemon.config.read().await.http.tcp.clone();
    let resource_manager = Arc::clone(&daemon.resource_manager);
    let listeners = Listeners {
        socket_path,
        tcp: tcp.bind_addr(),
        allowed_origins: tcp.allowed_origins.clone(),
    };
    let config = AuthConfig {
        dialog: ConsentDialog {
            product: &SUPER_STT,
            describe_scopes: super_stt_shared::consent::permissions_for_scopes,
        },
        keyring: crate::keyring::keyring(),
        resource_manager: Arc::clone(&resource_manager),
        allowed_origins: tcp.allowed_origins,
    };
    serve(listeners, resource_manager, shutdown_tx, move || {
        let auth = Auth::load(config)?;
        Ok(crate::daemon::http::v1::router(AppState::new(daemon, auth)))
    })
    .await
}

#[cfg(test)]
mod tests {
    /// The variable the tests and CI set is the one the shared auth code
    /// reads.
    #[test]
    fn auto_approve_env_is_the_one_the_engine_reads() {
        assert_eq!(
            super::AUTO_APPROVE_ENV,
            super::SUPER_STT.env(super_engine_daemon::auth::AUTO_APPROVE)
        );
    }
}
