// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::http_client::transport;
use super_stt_shared::registry::InstallAccepted;

/// `POST /registry/backends/install` — install a backend by its registry
/// `source` (e.g. `"github.com/super-stt/openai"`).
pub async fn install_by_source(source: &str) -> HttpResult<InstallAccepted> {
    let body = serde_json::json!({ "source": source });
    with_settings_token(move |socket, token| {
        let body = body.clone();
        async move {
            transport::post_json::<InstallAccepted>(
                socket,
                &token,
                "/registry/backends/install",
                &body,
            )
            .await
        }
    })
    .await
}

/// `POST /registry/backends/install` — install a backend by an arbitrary Git
/// repository URL (custom / out-of-registry install).
pub async fn install_by_repo_url(repo_url: &str) -> HttpResult<InstallAccepted> {
    let body = serde_json::json!({ "repo_url": repo_url });
    with_settings_token(move |socket, token| {
        let body = body.clone();
        async move {
            transport::post_json::<InstallAccepted>(
                socket,
                &token,
                "/registry/backends/install",
                &body,
            )
            .await
        }
    })
    .await
}

/// `POST /registry/backends/install` — install a backend by copying it from
/// a local directory (Import-from-dir). The daemon expects an absolute path.
pub async fn install_by_local_path(local_path: &str) -> HttpResult<InstallAccepted> {
    let body = serde_json::json!({ "local_path": local_path });
    with_settings_token(move |socket, token| {
        let body = body.clone();
        async move {
            transport::post_json::<InstallAccepted>(
                socket,
                &token,
                "/registry/backends/install",
                &body,
            )
            .await
        }
    })
    .await
}
