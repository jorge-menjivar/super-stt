// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline/{stage}/model` — the model a stage is pointed at.
//!
//! Read it, run it, stop it, or abandon a load still in flight. All of them are
//! scoped to the stage in the path: the stages provision independently, so the
//! Cancel button under a post-processor's progress must not abandon a
//! transcription model's download.
//!
//! Emptying the stage entirely — forgetting the backend with the model — is
//! [`super::stage::clear_stage_backend`], one level up the path.

use serde::Deserialize;

use crate::daemon::client::internal::response::{require_success, require_unit};
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::http_client::transport;

/// Which accelerator a stage's model runs on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct StageDevice {
    /// The stored preference: `cpu`, `gpu`, or `none` for a model that runs
    /// remotely. This is what a device picker shows as its current value.
    #[serde(default)]
    pub preference: String,
    /// What a `gpu` preference resolved to once the model loaded. `None` until
    /// a load has confirmed one, so it is never displayed as fact before then.
    #[serde(default)]
    pub resolved_accel: Option<String>,
}

/// One stage's model slot: what is selected, whether it is up, and the device
/// it runs on.
///
/// `model` is the *selection*, not the running instance — it survives an
/// unload, so the card can offer to load the same model again without the user
/// picking it a second time. `loaded` is what says whether it is up.
///
/// Only the fields the settings app consumes are modeled; serde ignores the
/// rest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct StageModel {
    /// `None` when the stage has a backend but no model picked.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub loaded: bool,
    /// `None` when nothing is selected.
    #[serde(default)]
    pub device: Option<StageDevice>,
}

impl StageModel {
    /// The accelerator the model is actually running on, or `None` when it is
    /// not running. What a card suffixes its "Active:" line with — the
    /// preference is not it, since a `gpu` choice can fall back to the CPU.
    #[must_use]
    pub fn running_device(&self) -> Option<&str> {
        if !self.loaded {
            return None;
        }
        self.device
            .as_ref()
            .and_then(|d| d.resolved_accel.as_deref())
            .filter(|d| !d.is_empty() && *d != "none")
    }
}

/// Wire envelope for `GET /pipeline/{stage}/model`.
#[derive(Debug, Clone, Deserialize)]
struct ModelEnvelope {
    model: SwitchPayload,
}

/// The switch sub-object, which only the download poller reads. The slot's own
/// fields are deserialized into [`StageModel`] by [`get_stage_model`]; serde
/// ignores them here.
#[derive(Debug, Clone, Deserialize)]
struct SwitchPayload {
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

fn model_path(stage: u32) -> String {
    format!("/pipeline/{stage}/model")
}

/// Read `stage`'s model slot (HTTP `GET /pipeline/{stage}/model`).
pub async fn get_stage_model(stage: u32) -> HttpResult<StageModel> {
    with_settings_token(move |socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, &model_path(stage)).await?,
            "get_stage_model",
        )?;
        // A daemon predating the endpoint answers without a slot; read that as
        // "nothing selected" rather than failing the whole settings load.
        Ok(resp
            .stage_model
            .map(|m| StageModel {
                model: m.model,
                loaded: m.loaded,
                device: m.device.map(|d| StageDevice {
                    preference: d.preference,
                    resolved_accel: d.resolved_accel,
                }),
            })
            .unwrap_or_default())
    })
    .await
}

/// The download `stage` has in flight, composed from its model slot's
/// `switch.download` sub-object. The polled counterpart of the
/// `download_progress` event, for the ticks a client may have missed.
pub async fn get_download_status(
    stage: u32,
) -> HttpResult<Option<super_stt_shared::models::protocol::DownloadProgress>> {
    with_settings_token(move |socket, token| async move {
        let status =
            transport::get_json::<ModelEnvelope>(socket, &token, &model_path(stage)).await?;
        let Some(switch) = status.model.switch else {
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
            // The stage that was asked: the slot reports only its own stage's
            // switch, so the answer belongs to the stage in the path.
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
            // The polled `switch` shape carries no error detail; failure text
            // arrives on the `download_progress` SSE event.
            error: None,
        }))
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
            let resp =
                transport::settings_post_no_timeout(socket, &token, &model_path(stage), &body)
                    .await?;
            require_unit(resp, "set_stage_model")
        }
    })
    .await
}

/// Stop running `stage`'s model, keeping the selection
/// (HTTP `DELETE /pipeline/{stage}/model`).
///
/// This is a card's Unload: the backend stays *and so does the model*, so
/// loading it again — onto another device, say — is one click rather than a
/// re-pick. [`super::stage::clear_stage_backend`] is the one that forgets the
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
