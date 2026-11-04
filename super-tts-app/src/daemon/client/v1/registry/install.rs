// SPDX-License-Identifier: GPL-3.0-only
//! `/registry/backend/install` — put a backend's files on disk.
//!
//! One path, three ways to name what to install, and the daemon accepts exactly
//! one of them per request. They are separate functions rather than one with an
//! enum because the caller always knows which it has — a registry row, a URL
//! the user pasted, a directory they picked — and the daemon answers
//! `400 bad_request` when more than one key is present.
//!
//! All three answer immediately with an accepted-install handle: the work runs
//! on, and progress arrives on the `daemon_status` event topic.

use crate::daemon::client::internal::session::with_settings_token;
use super_tts_shared::daemon::http_client::HttpResult;
use super_tts_shared::daemon::http_client::transport;
use super_tts_shared::registry::InstallAccepted;

/// Install a backend by its registry `source`
/// (e.g. `"github.com/super-tts/openai"`).
pub async fn install_by_source(source: &str) -> HttpResult<InstallAccepted> {
    let body = serde_json::json!({ "source": source });
    with_settings_token(move |socket, token| {
        let body = body.clone();
        async move {
            transport::post_json::<InstallAccepted>(
                socket,
                &token,
                "/registry/backend/install",
                &body,
            )
            .await
        }
    })
    .await
}

/// Install a backend from an arbitrary Git repository URL — the
/// custom/out-of-registry path, for a backend nobody has published.
pub async fn install_by_repo_url(repo_url: &str) -> HttpResult<InstallAccepted> {
    let body = serde_json::json!({ "repo_url": repo_url });
    with_settings_token(move |socket, token| {
        let body = body.clone();
        async move {
            transport::post_json::<InstallAccepted>(
                socket,
                &token,
                "/registry/backend/install",
                &body,
            )
            .await
        }
    })
    .await
}

/// Install a backend by copying it from a local directory — Import-from-dir,
/// the path a backend author uses on their own build. The daemon expects an
/// absolute path, since it resolves it in its own working directory and not the
/// app's.
pub async fn install_by_local_path(local_path: &str) -> HttpResult<InstallAccepted> {
    let body = serde_json::json!({ "local_path": local_path });
    with_settings_token(move |socket, token| {
        let body = body.clone();
        async move {
            transport::post_json::<InstallAccepted>(
                socket,
                &token,
                "/registry/backend/install",
                &body,
            )
            .await
        }
    })
    .await
}
