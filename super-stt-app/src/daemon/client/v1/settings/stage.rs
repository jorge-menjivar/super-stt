// SPDX-License-Identifier: GPL-3.0-only
//! The pipeline, addressed by stage position.
//!
//! A transcript passes through ordered stages: stage 1 turns audio into text,
//! every later stage rewrites what the one before it produced. Every stage
//! answers the same paths — `/pipeline/{stage}` selects the backend filling it
//! and reports what is running there, `/pipeline/{stage}/model` runs a model in
//! it — so there is one implementation here for all of them.
//!
//! It used to be one copy per stage, with the position baked into a `&str`
//! constant in each file. The copies drifted: stage 2's `set` kept the header
//! timeout that stage 1's documents skipping, which turned a first load's
//! download into a spurious failure. Taking the stage as a parameter is what
//! stops that happening again.

use serde::Deserialize;

use crate::daemon::client::internal::response::{require_success, require_unit};
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::http_client::transport;

/// One pipeline stage: which backend fills it, what is running, and the
/// progress of a load still in flight.
///
/// Only the fields the settings app consumes are modeled; serde ignores the
/// rest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct StageState {
    /// `None` when the stage is empty (no backend selected).
    #[serde(default)]
    pub source: Option<String>,
    /// `None` when the stage has a backend but no model picked.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub loaded: bool,
    /// The accelerator the loaded model runs on; `None` when nothing is loaded.
    #[serde(default)]
    pub device: Option<String>,
    /// The user's on/off choice, for stages that carry one separately from
    /// whether the model actually came up. Absent for stage 1, which has no
    /// switch of its own — read it through [`StageState::is_enabled`] rather
    /// than directly.
    #[serde(default)]
    pub enabled: Option<bool>,
}

impl StageState {
    /// The selected `(model, source)` pair, when the selection is complete.
    #[must_use]
    pub fn selection(&self) -> Option<(String, String)> {
        Some((self.model.clone()?, self.source.clone()?))
    }

    /// Whether this stage is meant to be running.
    ///
    /// A stage that carries its own on/off choice answers with it: it can be
    /// enabled while its model failed to load, and transcripts then pass
    /// through unprocessed, which is what the card reports. A stage without
    /// one is on exactly when it has a model up.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(self.loaded)
    }
}

/// Wire envelope for `GET /pipeline/{stage}`.
#[derive(Debug, Clone, Deserialize)]
struct StageEnvelope {
    stage: StageSwitchPayload,
}

/// The switch sub-object, which only the download poller reads. The stage's
/// own fields are deserialized into [`StageState`] by [`get_stage`]; serde
/// ignores them here.
#[derive(Debug, Clone, Deserialize)]
struct StageSwitchPayload {
    #[serde(default)]
    switch: Option<StageSwitch>,
}

#[derive(Debug, Clone, Deserialize)]
struct StageSwitch {
    phase: String,
    target: serde_json::Value,
    started_at: Option<String>,
    download: Option<StageDownload>,
}

#[derive(Debug, Clone, Deserialize)]
struct StageDownload {
    current_file: String,
    file_index: usize,
    total_files: usize,
    bytes_downloaded: u64,
    total_bytes: u64,
    percentage: f32,
    eta_seconds: Option<u64>,
}

/// The path a stage answers on, and the path its model answers on.
fn stage_path(stage: u32) -> String {
    format!("/pipeline/{stage}")
}

fn stage_model_path(stage: u32) -> String {
    format!("/pipeline/{stage}/model")
}

async fn fetch(socket: std::path::PathBuf, token: &str, stage: u32) -> HttpResult<StageEnvelope> {
    transport::get_json::<StageEnvelope>(socket, token, &stage_path(stage)).await
}

/// Read `stage`'s state (HTTP `GET /pipeline/{stage}`).
pub async fn get_stage(stage: u32) -> HttpResult<StageState> {
    with_settings_token(move |socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, &stage_path(stage)).await?,
            "get_pipeline_stage",
        )?;
        // A daemon that predates the pipeline omits the stage; read that as
        // "empty, nothing selected" rather than failing the settings load.
        Ok(resp
            .stage
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default())
    })
    .await
}

/// Select the backend filling `stage` (HTTP `POST /pipeline/{stage}`).
///
/// Records which backend fills the stage and unloads a foreign model — it does
/// NOT load one. Pair with [`set_stage_model`] to also run a model.
pub async fn set_stage_backend(stage: u32, source: String) -> HttpResult<()> {
    with_settings_token(move |socket, token| {
        let source = source.clone();
        async move {
            let resp = transport::settings_post(
                socket,
                &token,
                &stage_path(stage),
                &serde_json::json!({ "source": source }),
            )
            .await?;
            require_unit(resp, "set_stage_backend")
        }
    })
    .await
}

/// Empty `stage`, forgetting the model with it
/// (HTTP `DELETE /pipeline/{stage}`).
///
/// This is a card's Deselect. [`unload_stage_model`] is the softer one that
/// keeps the backend.
pub async fn clear_stage_backend(stage: u32) -> HttpResult<()> {
    with_settings_token(move |socket, token| async move {
        let resp = transport::settings_delete(socket, &token, &stage_path(stage)).await?;
        require_unit(resp, "clear_stage_backend")
    })
    .await
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
            let resp = transport::settings_post_no_timeout(
                socket,
                &token,
                &stage_model_path(stage),
                &body,
            )
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
/// pick away. [`clear_stage_backend`] is the one that forgets the selection.
pub async fn unload_stage_model(stage: u32) -> HttpResult<()> {
    with_settings_token(move |socket, token| async move {
        let resp = transport::settings_delete(socket, &token, &stage_model_path(stage)).await?;
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

/// The download `stage` has in flight, composed from that stage's
/// `switch.download` sub-object. The polled counterpart of the
/// `download_progress` event, for the ticks a client may have missed.
pub async fn get_download_status(
    stage: u32,
) -> HttpResult<Option<super_stt_shared::models::protocol::DownloadProgress>> {
    with_settings_token(move |socket, token| async move {
        let status = fetch(socket, &token, stage).await?;
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
        Ok(Some(super_stt_shared::models::protocol::DownloadProgress {
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
            // The polled `stage.switch` shape carries no error detail; failure
            // text arrives on the `download_progress` SSE event.
            error: None,
        }))
    })
    .await
}
