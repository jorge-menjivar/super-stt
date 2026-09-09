// SPDX-License-Identifier: GPL-3.0-only
//! `POST /registry/backend/preview` — what a source would install, without
//! installing it. Fills the Add-a-backend drawer's preview so the operator
//! sees the backend before committing to it.

use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::http_client::transport;
use super_stt_shared::registry::PreviewResponse;

/// Resolve `repo_url` and describe the backend it would install.
pub async fn preview_by_repo_url(repo_url: &str) -> HttpResult<PreviewResponse> {
    preview(serde_json::json!({ "repo_url": repo_url })).await
}

/// Resolve `local_path` and describe the backend it would install.
pub async fn preview_by_local_path(local_path: &str) -> HttpResult<PreviewResponse> {
    preview(serde_json::json!({ "local_path": local_path })).await
}

async fn preview(body: serde_json::Value) -> HttpResult<PreviewResponse> {
    with_settings_token(move |socket, token| {
        let body = body.clone();
        async move {
            transport::post_json::<PreviewResponse>(
                socket,
                &token,
                "/registry/backend/preview",
                &body,
            )
            .await
        }
    })
    .await
}
