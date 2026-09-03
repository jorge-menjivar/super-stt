// SPDX-License-Identifier: GPL-3.0-only
//! Stage 1 of the pipeline — the model that turns audio into text — plus the
//! flat `/models` catalog.
//!
//! A transcript passes through ordered stages, and every stage answers the same
//! paths: `/pipeline/{stage}` selects the backend filling it,
//! `/pipeline/{stage}/model` runs a model in it. These wrappers are stage 1.

use crate::daemon::client::internal::response::{require_message, require_success};
use crate::daemon::client::internal::session::with_settings_token;
use serde::Deserialize;
use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::http_client::transport;

/// Transcription is stage 1 of the pipeline. Named here so the position appears
/// once, and a re-ordering is a one-line change.
const STT_STAGE: &str = "/pipeline/1";
const STT_STAGE_MODEL: &str = "/pipeline/1/model";

// Only the stage fields the settings app consumes are modeled; serde ignores
// the rest.

/// Wire shape returned by `GET /pipeline/{stage}`.
#[derive(Debug, Clone, Deserialize)]
pub struct StageStatus {
    pub stage: StagePayload,
}

/// One pipeline stage: which backend fills it, what is running, and the
/// progress of a load that is still in flight.
#[derive(Debug, Clone, Deserialize)]
pub struct StagePayload {
    pub model: Option<String>,
    pub source: Option<String>,
    /// The accelerator the loaded model is running on; `None` when nothing
    /// is loaded.
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub switch: Option<ActiveModelSwitch>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActiveModelSwitch {
    pub phase: String,
    pub target: serde_json::Value,
    pub started_at: Option<String>,
    pub download: Option<ActiveModelDownload>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActiveModelDownload {
    pub current_file: String,
    pub file_index: usize,
    pub total_files: usize,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub percentage: f32,
    pub eta_seconds: Option<u64>,
}

async fn fetch_stage(socket: std::path::PathBuf, token: &str) -> HttpResult<StageStatus> {
    transport::get_json::<StageStatus>(socket, token, STT_STAGE).await
}

/// The same read for any stage, addressed by position. Each stage reports only
/// its own switch, so a caller asking about one stage never sees another's.
async fn fetch_stage_at(
    socket: std::path::PathBuf,
    token: &str,
    stage: u32,
) -> HttpResult<StageStatus> {
    transport::get_json::<StageStatus>(socket, token, &format!("/pipeline/{stage}")).await
}

/// Get current loaded model from daemon as `(name source)`
/// (HTTP `GET /pipeline/1`).
pub async fn get_current_model() -> HttpResult<(String, String)> {
    with_settings_token(|socket, token| async move {
        let current = fetch_stage(socket, &token).await?.stage;
        // Idle daemon (no model loaded) is a valid state: report an empty
        // selection rather than erroring, so the UI shows nothing selected.
        let Some(model) = current.model else {
            return Ok((String::new(), String::new()));
        };

        let source = current.source.unwrap_or_default();
        Ok((model, source))
    })
    .await
}

/// The accelerator the loaded stage-1 model is running on (HTTP
/// `GET /pipeline/1`, `stage.device`) — what the active card's `· device`
/// suffix shows. `None` when nothing is loaded.
pub async fn get_current_device() -> HttpResult<Option<String>> {
    with_settings_token(|socket, token| async move {
        Ok(fetch_stage(socket, &token).await?.stage.device)
    })
    .await
}

/// The download `stage` has in flight, composed from that stage's
/// `switch.download` sub-object. The polled counterpart of the
/// `download_progress` event, for the ticks a client may have missed.
pub async fn get_download_status(
    stage: u32,
) -> HttpResult<Option<super_stt_shared::models::protocol::DownloadProgress>> {
    with_settings_token(move |socket, token| async move {
        let status = fetch_stage_at(socket, &token, stage).await?;
        let Some(switch) = status.stage.switch else {
            return Ok(None);
        };
        let Some(download) = switch.download else {
            return Ok(None);
        };
        let target_field = |key: &str| {
            switch
                .target
                .get(key)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        let progress = super_stt_shared::models::protocol::DownloadProgress {
            model_name: target_field("model"),
            source: target_field("source"),
            // The stage that was asked: `GET /pipeline/{stage}` reports only
            // its own switch, so the answer belongs to the stage in the path.
            stage,
            current_file: download.current_file,
            file_index: download.file_index,
            total_files: download.total_files,
            bytes_downloaded: download.bytes_downloaded,
            total_bytes: download.total_bytes,
            percentage: download.percentage,
            status: switch.phase,
            started_at: switch.started_at.unwrap_or_default(),
            eta_seconds: download.eta_seconds,
            // The polled `stage.switch` shape carries no error detail;
            // failure text arrives on the `download_progress` SSE event.
            error: None,
        };
        Ok(Some(progress))
    })
    .await
}

/// Set/switch to a different model (HTTP `POST /pipeline/1/model`).
pub async fn set_model(model: String, source: String) -> HttpResult<String> {
    let source_str = source;
    with_settings_token(move |socket, token| {
        let model = model.clone();
        let source_str = source_str.clone();
        async move {
            let mut body = serde_json::json!({ "model": model });
            body["source"] = serde_json::Value::String(source_str);
            // No header timeout: the daemon only responds once the switch
            // finishes (provisioning may stream multi-GB weights first), and
            // the fixed timeout would drop the connection and cancel the load.
            // Progress/outcome is observed via the `download_progress` SSE topic.
            let resp =
                transport::settings_post_no_timeout(socket, &token, STT_STAGE_MODEL, &body).await?;
            require_message(resp, "set_model")
        }
    })
    .await
}

/// List all available models from daemon (HTTP `GET /models`).
pub async fn list_available_models() -> HttpResult<Vec<(String, String)>> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, "/models").await?,
            "list_models",
        )?;
        Ok(resp.available_models.unwrap_or_default())
    })
    .await
}

/// Abandon the load `stage` has in flight
/// (HTTP `POST /pipeline/{stage}/model/cancel`).
///
/// Addressed to a stage because the stages provision independently: the Cancel
/// button under a post-processor's progress must not abandon a transcription
/// model's download.
pub async fn cancel_download(stage: u32) -> HttpResult<String> {
    let path = format!("/pipeline/{stage}/model/cancel");
    with_settings_token(move |socket, token| {
        let path = path.clone();
        async move {
            let resp =
                transport::settings_post(socket, &token, &path, &serde_json::json!({})).await?;
            require_message(resp, "cancel_download")
        }
    })
    .await
}

/// Unload the currently loaded model (HTTP `DELETE /pipeline/1/model`). The
/// stage keeps its backend, so the user can pick another of its models. Use
/// [`clear_active_backend`] to empty the stage entirely.
pub async fn unload_active_model() -> HttpResult<String> {
    with_settings_token(|socket, token| async move {
        let resp = transport::settings_delete(socket, &token, STT_STAGE_MODEL).await?;
        require_message(resp, "unload_active_model")
    })
    .await
}
