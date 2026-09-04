// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline/{stage}/model` — the model running in a stage.
//!
//! Run one, stop it, or abandon a load still in flight. All three are scoped to
//! the stage in the path: the stages provision independently, so the Cancel
//! button under a post-processor's progress must not abandon a transcription
//! model's download.
//!
//! Emptying the stage entirely — forgetting the backend with the model — is
//! [`super::stage::clear_stage_backend`], one level up the path.

use crate::daemon::client::internal::response::require_unit;
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::http_client::transport;

fn model_path(stage: u32) -> String {
    format!("/pipeline/{stage}/model")
}

/// Run `model` in `stage` (HTTP `POST /pipeline/{stage}/model`).
///
/// `source` is optional: omitted, the daemon resolves the model against the
/// backend already filling the stage.
pub async fn set_stage_model(stage: u32, model: String, source: Option<String>) -> HttpResult<()> {
    let mut body = serde_json::json!({ "model": model });
    if let Some(source) = source {
        body["source"] = serde_json::Value::String(source);
    }
    with_settings_token(move |socket, token| {
        let body = body.clone();
        async move {
            // No header timeout: the daemon answers only once the load
            // finishes, and provisioning may stream multi-GB weights first. The
            // fixed timeout would drop the connection and report a failure the
            // daemon never had. Progress and outcome arrive on the
            // `download_progress` SSE topic instead.
            let resp =
                transport::settings_post_no_timeout(socket, &token, &model_path(stage), &body)
                    .await?;
            require_unit(resp, "set_stage_model")
        }
    })
    .await
}

/// Stop running `stage`'s model, keeping its backend selected
/// (HTTP `DELETE /pipeline/{stage}/model`).
///
/// This is a card's Unload: the backend stays, so another of its models is one
/// pick away. [`super::stage::clear_stage_backend`] is the one that forgets the
/// selection.
pub async fn unload_stage_model(stage: u32) -> HttpResult<()> {
    with_settings_token(move |socket, token| async move {
        let resp = transport::settings_delete(socket, &token, &model_path(stage)).await?;
        require_unit(resp, "unload_stage_model")
    })
    .await
}

/// Abandon the load `stage` has in flight
/// (HTTP `POST /pipeline/{stage}/model/cancel`).
///
/// Addressed to a stage because the stages provision independently: the Cancel
/// button under a post-processor's progress must not abandon a transcription
/// model's download.
pub async fn cancel_download(stage: u32) -> HttpResult<()> {
    let path = format!("/pipeline/{stage}/model/cancel");
    with_settings_token(move |socket, token| {
        let path = path.clone();
        async move {
            let resp =
                transport::settings_post(socket, &token, &path, &serde_json::json!({})).await?;
            require_unit(resp, "cancel_download")
        }
    })
    .await
}
