// SPDX-License-Identifier: GPL-3.0-only
//! Daemon-facing operations for the applet.
//!
//! Liveness probes (`/ping`) run over the HTTP protocol using a
//! widget-scope session token cached by
//! `super_stt_shared::daemon::session`. The applet's main subscription
//! to `GET /events` is handled separately by
//! `super_stt_shared::daemon::widget_subscription::run_widget_subscription`.

use std::path::PathBuf;
use super_stt_shared::daemon::http_client;
use super_stt_shared::daemon::session::{self, AppId};

const APP_ID: AppId = AppId("super-stt-cosmic-applet");
const APP_NAME: &str = "Super STT Applet";
const SCOPE: &str = "widget";

/// What the applet's update loop expects from `ping_daemon_with_status`.
/// In the legacy protocol the daemon could report
/// `connection_active = false` separately from a successful ping; the
/// HTTP `/ping` route is just 200-or-not, so a successful response
/// always implies `connection_active = true`.
pub struct PingResponse {
    pub message: String,
    pub connection_active: bool,
}

/// Run an HTTP-protocol operation with the cached widget-scope token.
/// On `invalid_session` the cache is invalidated and the operation
/// retries once with a fresh consent flow. Delegates the retry
/// matching to [`session::with_token`] in the shared crate.
async fn with_widget_token<F, Fut, T>(socket_path: PathBuf, op: F) -> Result<T, String>
where
    F: Fn(PathBuf, String) -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let socket_for_op = socket_path.clone();
    session::with_token(socket_path, APP_ID, APP_NAME, SCOPE, move |token| {
        op(socket_for_op.clone(), token)
    })
    .await
}

/// Ping the daemon to check it's reachable. Returns the daemon's
/// `message` field (typically `"pong"`).
///
/// # Errors
///
/// Returns an error string if the daemon HTTP listener is unreachable,
/// the consent flow fails, or the token is no longer valid after one
/// retry.
pub async fn ping_daemon(socket_path: PathBuf) -> Result<String, String> {
    with_widget_token(socket_path, |sock, token| async move {
        http_client::ping(sock, &token).await.map_err(String::from)
    })
    .await
}

/// Ping the daemon and report whether the connection should be
/// considered active. Mirrors the legacy two-field response; under
/// HTTP, a successful ping always means `connection_active = true`.
///
/// # Errors
///
/// Same conditions as [`ping_daemon`].
pub async fn ping_daemon_with_status(socket_path: PathBuf) -> Result<PingResponse, String> {
    let message = ping_daemon(socket_path).await?;
    Ok(PingResponse {
        message,
        connection_active: true,
    })
}
