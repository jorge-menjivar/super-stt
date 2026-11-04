// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline/{stage}/model` — the model a stage is pointed at.
//!
//! Read it, run it, stop it, abandon a load still in flight, list what it could
//! be, or re-instantiate it in place. All of them are scoped to the stage in
//! the path: stages provision independently, so a Cancel under one stage's
//! progress bar must not abandon another stage's download.
//!
//! Emptying the stage entirely — forgetting the backend along with the model —
//! is [`super::stage::clear_stage_backend`], one level up the path.
//!
//! The slot carries no `source`. The backend belongs to the stage, not to the
//! model, so a caller that needs the pair reads it through
//! [`super::StageState::selection`] rather than expecting both halves from one
//! request.

use serde::Deserialize;

use crate::daemon::client::internal::response::{require_success, require_unit};
use crate::daemon::client::internal::session::with_settings_token;
use super_tts_shared::daemon::http_client::HttpResult;
use super_tts_shared::daemon::http_client::transport;

/// Which accelerator a stage's model runs on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct StageDevice {
    /// The stored preference: `cpu`, `gpu`, or `none` for a model that
    /// synthesizes remotely and therefore has no local device. This is what a
    /// device picker shows as its current value.
    #[serde(default)]
    pub preference: String,
    /// What a `gpu` preference resolved to once the model loaded. `None` until
    /// a load has confirmed one, so it is never displayed as fact before then —
    /// a `gpu` choice can still fall back to the CPU.
    #[serde(default)]
    pub resolved_accel: Option<String>,
}

/// One stage's model slot: what is selected, whether it is up, and the device
/// it runs on.
///
/// `model` is the *selection*, not the running instance — it survives an
/// unload, so the card can offer to load the same model again, onto another
/// device say, without the user picking it a second time. `loaded` is what says
/// whether it is up, and what `POST /speak` requires.
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
///
/// Parsed here rather than off the shared `DaemonResponse`, whose fields happen
/// to match most envelope keys by name. That coupling is invisible and fails
/// soft: this endpoint's key is `model` where the shared field is
/// `stage_model`, so reading it that way yields `None` on every call — no
/// error, just a card that comes up with nothing selected after a restart the
/// daemon had remembered perfectly well.
#[derive(Debug, Clone, Deserialize)]
struct ModelEnvelope {
    #[serde(default)]
    status: String,
    #[serde(default)]
    model: SlotPayload,
}

/// The slot as it arrives. `switch` is the download poller's; the rest is
/// [`StageModel`]'s.
#[derive(Debug, Clone, Default, Deserialize)]
struct SlotPayload {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    loaded: bool,
    #[serde(default)]
    device: Option<StageDevice>,
    #[serde(default)]
    switch: Option<StageSwitch>,
}

/// The load this stage has in flight. Every field is optional here even where
/// the daemon always fills it: this shape rides along with the selection a card
/// draws itself from, and a progress sub-object that arrived unexpectedly empty
/// should cost the progress bar, not the whole card.
#[derive(Debug, Clone, Deserialize)]
struct StageSwitch {
    #[serde(default)]
    phase: String,
    #[serde(default)]
    target: serde_json::Value,
    #[serde(default)]
    started_at: Option<String>,
    #[serde(default)]
    download: Option<StageDownload>,
    #[serde(default)]
    load: Option<super_tts_shared::models::protocol::LoadProgress>,
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
        let envelope =
            transport::get_json::<ModelEnvelope>(socket, &token, &model_path(stage)).await?;
        if envelope.status != "success" {
            return Err(super_tts_shared::daemon::http_client::HttpError::Other(
                format!("get_stage_model: daemon answered {}", envelope.status),
            ));
        }
        Ok(StageModel {
            model: envelope.model.model,
            loaded: envelope.model.loaded,
            device: envelope.model.device,
        })
    })
    .await
}

/// The download `stage` has in flight, composed from its model slot's
/// `switch.download` sub-object (HTTP `GET /pipeline/{stage}/model`).
///
/// The polled counterpart of the `download_progress` event, for the ticks a
/// client may have missed — a page opened mid-download has no history to catch
/// up on and would otherwise show a stalled bar until the next event.
pub async fn get_download_status(
    stage: u32,
) -> HttpResult<Option<super_tts_shared::models::protocol::DownloadProgress>> {
    with_settings_token(move |socket, token| async move {
        let status =
            transport::get_json::<ModelEnvelope>(socket, &token, &model_path(stage)).await?;
        let Some(switch) = status.model.switch else {
            return Ok(None);
        };
        let Some(download) = switch.download else {
            return Ok(None);
        };
        let model_name = switch
            .target
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Ok(Some(super_tts_shared::models::protocol::DownloadProgress {
            model_name,
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
            load: switch.load,
        }))
    })
    .await
}

/// Run `model` in `stage` (HTTP `POST /pipeline/{stage}/model`).
///
/// `source` is optional: omitted, the daemon resolves the model against the
/// backend already filling the stage. Naming one also selects that backend, so
/// a flat model picker can go straight to a load. It is the key's *absence*
/// that asks for resolution — a `null` would read as a request to load from no
/// backend at all — which is why it is inserted rather than always sent.
pub async fn set_stage_model(stage: u32, model: String, source: Option<String>) -> HttpResult<()> {
    let mut body = serde_json::json!({ "model": model });
    if let Some(source) = source {
        body["source"] = serde_json::Value::String(source);
    }
    with_settings_token(move |socket, token| {
        let body = body.clone();
        async move {
            // No header timeout: the daemon answers only once the switch
            // finishes, and provisioning may stream multi-GB weights first. The
            // fixed timeout would drop the connection and cancel the
            // daemon-side load. Progress and outcome arrive on the
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
/// This is a card's Unload: the device memory is freed, the backend stays *and
/// so does the model*, so loading it again — onto another device, say — is one
/// click rather than a re-pick. [`super::stage::clear_stage_backend`] is the
/// one that forgets the selection.
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
/// Addressed to a stage because stages provision independently: cancelling one
/// is not a licence to abandon another's download. A stage with nothing in
/// flight answers `409`, which surfaces here as an error rather than a silent
/// success — a Cancel that reported success without cancelling anything is
/// worse than one that says there was nothing to cancel.
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

/// Re-instantiate `stage`'s model in place
/// (HTTP `POST /pipeline/{stage}/model/reload`).
///
/// Same model, same device, brought back up so it picks up a changed secret or
/// option without the caller unloading and loading by hand. Nothing is
/// re-downloaded, and a stage with nothing loaded succeeds having done nothing.
///
/// Rarely needed: writing an option or a secret already reloads every stage
/// running a model from that backend. This is the explicit button for the case
/// where something changed outside the daemon's view — a keyring edited by
/// another tool, say.
pub async fn reload_stage_model(stage: u32) -> HttpResult<()> {
    let path = format!("/pipeline/{stage}/model/reload");
    with_settings_token(move |socket, token| {
        let path = path.clone();
        async move {
            // Same reasoning as `set_stage_model`: the daemon answers only once
            // the model is back up, and a reload of a large model can outlast
            // the fixed header timeout.
            let resp =
                transport::settings_post_no_timeout(socket, &token, &path, &serde_json::json!({}))
                    .await?;
            require_unit(resp, "reload_stage_model")
        }
    })
    .await
}

/// The models `stage` can run (HTTP `GET /pipeline/{stage}/model/list`).
///
/// `[name, source]` pairs from the backend filling the stage, filtered to the
/// ones that serve this stage's role — the exact pair
/// [`set_stage_model`] accepts. Empty when the stage has no backend selected,
/// which reads correctly as "choose a backend first".
///
/// Not the same list as [`crate::daemon::client::v1::backends::list_backends`]:
/// that is every installed backend's whole catalog, and a model from another
/// backend cannot load here without changing the stage's backend too. Offering
/// one hands the user a pick that does something other than what the list
/// implied.
pub async fn list_stage_models(stage: u32) -> HttpResult<Vec<(String, String)>> {
    let path = format!("/pipeline/{stage}/model/list");
    with_settings_token(move |socket, token| {
        let path = path.clone();
        async move {
            let resp = require_success(
                transport::settings_get(socket, &token, &path).await?,
                "list_stage_models",
            )?;
            Ok(resp.available_models.unwrap_or_default())
        }
    })
    .await
}
