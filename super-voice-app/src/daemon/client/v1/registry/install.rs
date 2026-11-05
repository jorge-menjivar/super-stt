// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::client::internal::session::with_settings_token;
use super_voice_shared::daemon::http_client::HttpResult;
use super_voice_shared::daemon::http_client::transport;
use super_voice_shared::registry::InstallAccepted;

/// `POST /registry/backend/install` — install a backend by its registry
/// `source` (e.g. `"github.com/super-stt/openai"`).
///
/// `accel` asks for the build of one framework (`"cuda"`) rather than the one
/// the daemon ranks best for this machine. Installing over an installed
/// backend replaces its build and keeps its downloaded models.
pub async fn install_by_source(source: &str, accel: Option<&str>) -> HttpResult<InstallAccepted> {
    let mut body = serde_json::json!({ "source": source });
    if let Some(accel) = accel {
        body["accel"] = serde_json::Value::from(accel);
    }
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

/// `POST /registry/backend/install` — install a backend by an arbitrary Git
/// repository URL (custom / out-of-registry install).
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

/// `POST /registry/backend/install` — install a backend by copying it from
/// a local directory (Import-from-dir). The daemon expects an absolute path.
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
