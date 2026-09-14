// SPDX-License-Identifier: GPL-3.0-only
//! Session-token machinery for every app→daemon call.
//!
//! One cached token (minted via the daemon consent popup) authorizes
//! everything the settings app does. [`with_settings_token`] runs an
//! operation with that token and, on `invalid_session`, drops the cache
//! and retries once with fresh consent.

use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::session::{self, AppId};
use super_stt_shared::validation::get_http_socket_path;

/// Scope set the settings app requests. One cached token covers
/// everything the app does: config + registry (`settings`), the
/// test-recording panel (`transcribe`), and the `/events` subscription
/// (`recording_events` for the badge, `audio_visualization` for the
/// meter, `daemon_status` for model-switch / download / install progress).
pub(crate) const SETTINGS_SCOPES: &[&str] = &[
    "settings",
    "secrets",
    "transcribe",
    "recording_events",
    "audio_visualization",
    "daemon_status",
];
pub(crate) const APP_NAME: &str = "Super STT Settings App";
pub(crate) const APP_ID_NAME: AppId = AppId("super-stt-app");

/// The socket every app→daemon call connects on.
///
/// One resolver rather than a `get_http_socket_path()` per call site: the
/// streaming path in [`crate::daemon::client::v1::transcribe`] does not go
/// through [`with_settings_token`], and both have to reach the same daemon.
/// It is also the single seam the wrapper tests redirect at a fake daemon.
pub(crate) fn socket_path() -> std::path::PathBuf {
    #[cfg(test)]
    if let Some(path) = test_socket::get() {
        return path;
    }
    get_http_socket_path()
}

/// Test-only redirection of [`socket_path`], compiled out of the shipped
/// binary. Set while a `fake_daemon::FakeDaemon` is alive; that guard holds an
/// exclusive lock for its lifetime, so only one test at a time observes it.
#[cfg(test)]
pub(crate) mod test_socket {
    use std::path::PathBuf;
    use std::sync::Mutex;

    static SLOT: Mutex<Option<PathBuf>> = Mutex::new(None);

    pub(crate) fn get() -> Option<PathBuf> {
        SLOT.lock().expect("test socket slot").clone()
    }

    pub(crate) fn set(path: Option<PathBuf>) {
        *SLOT.lock().expect("test socket slot") = path;
    }
}

/// Run an HTTP-protocol operation with the cached settings-scope token.
/// On `invalid_session` the cache is invalidated and the operation
/// retries once with a fresh consent flow.
pub(crate) async fn with_settings_token<F, Fut, T>(op: F) -> HttpResult<T>
where
    F: Fn(std::path::PathBuf, String) -> Fut,
    Fut: std::future::Future<Output = HttpResult<T>>,
{
    let socket = socket_path();
    let socket_for_op = socket.clone();
    session::with_token(
        socket,
        APP_ID_NAME,
        APP_NAME,
        SETTINGS_SCOPES,
        move |token| op(socket_for_op.clone(), token),
    )
    .await
}
