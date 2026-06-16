// SPDX-License-Identifier: GPL-3.0-only
//! HTTP client for the daemon protocol.
//!
//! The transport is HTTP/1.1 over a Unix domain socket
//! (`super_stt_shared::validation::get_http_socket_path()`). Each request
//! opens a fresh `tokio::net::UnixStream`, runs `hyper::client::conn::http1`
//! over it, and parses the JSON response into a `DaemonResponse`.
//!
//! Authentication is per-request: callers pass a session token (obtained
//! from [`crate::daemon::session::obtain`]) and this module attaches it
//! as `Authorization: Bearer <token>` on every call except
//! [`auth_request`]. On 401 the daemon's `data.reason` is surfaced so the
//! caller can `session::forget` + re-`obtain`.

mod internal;
mod v1;

pub use internal::error::{HttpError, HttpResult};

pub use v1::auth::request::{AuthOk, auth_request};
pub use v1::auth::status::{AuthStatusInfo, auth_status};
pub use v1::events::{WidgetEvent, events_stream};
pub use v1::health::{ping, status};
pub use v1::transcribe::{
    TranscribeEvent, TranscribeOptions, transcribe, transcribe_stop, transcribe_stream,
};

/// Public transport surface for downstream clients that compose their own
/// per-scope endpoint wrappers (e.g. the settings app). Returns
/// [`HttpError`] on transport/auth failure; `401` becomes
/// [`HttpError::InvalidSession`].
pub mod transport {
    pub use super::internal::transport::{
        delete_json, get_json, post_json, settings_delete, settings_get, settings_post,
        settings_post_no_timeout,
    };
}
