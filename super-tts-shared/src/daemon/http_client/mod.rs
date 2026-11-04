// SPDX-License-Identifier: GPL-3.0-only
//! HTTP client for the daemon protocol: `super_engine_client::http_client`,
//! which Super TTS shares with Super STT, plus Super TTS's own endpoints.
//!
//! The transport is HTTP/1.1 over the daemon's Unix socket
//! (`super_tts_shared::validation::get_http_socket_path()`). Authentication is
//! per-request: callers pass a session token (obtained from
//! [`crate::daemon::session::obtain`]) and it is attached as
//! `Authorization: Bearer <token>` on every call except [`auth_request`].

mod speak;

use std::path::PathBuf;

pub use speak::{speak, speak_stop};
pub use super_engine_client::http_client::{
    AuthOk, AuthStatusInfo, HttpError, HttpResult, WidgetEvent, auth_request, auth_status,
    events_stream, ping,
};

use crate::models::protocol::DaemonResponse;

/// `GET /status` — current model + device.
///
/// # Errors
/// Returns an error if the daemon HTTP listener isn't reachable or the
/// response can't be parsed.
pub async fn status(socket_path: PathBuf, token: &str) -> HttpResult<DaemonResponse> {
    super_engine_client::http_client::status(socket_path, token).await
}

/// Public transport surface for downstream clients that compose their own
/// per-scope endpoint wrappers (e.g. the settings app): the engine's, plus
/// the `settings_*` calls that read Super TTS's [`DaemonResponse`]. Returns
/// [`HttpError`] on transport/auth failure; `401` becomes
/// [`HttpError::InvalidSession`].
pub mod transport {
    use std::path::PathBuf;

    pub use super_engine_client::http_client::transport::*;

    use super::HttpResult;
    use crate::models::protocol::DaemonResponse;

    /// `GET <path>` → `DaemonResponse`. The standard settings read.
    ///
    /// # Errors
    /// Returns [`super::HttpError::InvalidSession`] on `401`;
    /// [`super::HttpError::Other`] on connection, HTTP, or parse failure.
    pub async fn settings_get(
        socket_path: PathBuf,
        token: &str,
        path: &str,
    ) -> HttpResult<DaemonResponse> {
        get_json(socket_path, token, path).await
    }

    /// `POST <path>` with a JSON body → `DaemonResponse`. The standard
    /// settings write.
    ///
    /// # Errors
    /// Returns [`super::HttpError::InvalidSession`] on `401`;
    /// [`super::HttpError::Other`] on connection, HTTP, body encoding, or
    /// parse failure.
    pub async fn settings_post(
        socket_path: PathBuf,
        token: &str,
        path: &str,
        body: &serde_json::Value,
    ) -> HttpResult<DaemonResponse> {
        post_json(socket_path, token, path, body).await
    }

    /// Like [`settings_post`] but without the fixed header timeout — for a
    /// long-running write whose response the daemon only sends once the work
    /// completes (notably `POST /active_model`, a model switch that may stream
    /// multi-GB weights first). See [`post_json_no_timeout`].
    ///
    /// # Errors
    /// Returns [`super::HttpError::InvalidSession`] on `401`;
    /// [`super::HttpError::Other`] on connection, HTTP, body encoding, or
    /// parse failure.
    pub async fn settings_post_no_timeout(
        socket_path: PathBuf,
        token: &str,
        path: &str,
        body: &serde_json::Value,
    ) -> HttpResult<DaemonResponse> {
        post_json_no_timeout(socket_path, token, path, body).await
    }

    /// `DELETE <path>` → `DaemonResponse`.
    ///
    /// # Errors
    /// Returns [`super::HttpError::InvalidSession`] on `401`;
    /// [`super::HttpError::Other`] on connection, HTTP, or parse failure.
    pub async fn settings_delete(
        socket_path: PathBuf,
        token: &str,
        path: &str,
    ) -> HttpResult<DaemonResponse> {
        delete_json(socket_path, token, path).await
    }
}
