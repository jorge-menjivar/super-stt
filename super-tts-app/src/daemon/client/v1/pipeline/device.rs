// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline/{stage}/model/{model}/device` — the device a model runs on.
//!
//! The preference is per model, not per daemon. That is the whole reason this
//! replaced the global `/active_device`: a small preset-voice model runs fine
//! on the CPU while the cloning model beside it needs the GPU, and one global
//! setting meant choosing for one overwrote the choice for the other.
//!
//! It is addressed *through* the stage because the stage is what resolves a
//! bare model name to the backend serving it — two backends may serve the same
//! model name, and they are not the same model. A change for the model the
//! stage is running is a reload; for any other it is a note for the next load,
//! which is what lets a card set the device *before* Load without the daemon
//! loading twice.
//!
//! GPU memory is served separately by
//! [`crate::daemon::client::v1::gpu_info`], which reports the hardware without
//! regard to what any model supports.

use serde::Deserialize;

use crate::daemon::client::internal::response::{require_success, require_unit};
use crate::daemon::client::internal::session::with_settings_token;
use super_tts_shared::daemon::http_client::HttpResult;
use super_tts_shared::daemon::http_client::transport;

fn enc(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

/// What the daemon reports for one model's device.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ModelDevice {
    /// The `cpu`/`gpu` preference the model loads with — or `none` for an
    /// online model, which has no local device.
    pub device: String,
    /// The accelerator the model is actually on (`cuda`, `rocm`, `metal`,
    /// `vulkan`, `cpu`) when it is loaded; `None` before a `gpu` choice has
    /// resolved to anything.
    pub resolved_accel: Option<String>,
    /// The devices this install can offer the model on this host.
    pub available_devices: Vec<String>,
}

fn device_path(stage: u32, model: &str) -> String {
    format!("/pipeline/{stage}/model/{}/device", enc(model))
}

/// Read a model's device preference, what it resolved to, and what this host
/// can offer it (HTTP `GET /pipeline/{stage}/model/{model}/device`).
///
/// Everything a device control needs for its current value, in one request. It
/// answers whether or not the model is loaded, which is how a card shows the
/// device a model will load onto before anyone presses Load.
pub async fn get_model_device(stage: u32, model: String) -> HttpResult<ModelDevice> {
    let path = device_path(stage, &model);
    with_settings_token(move |socket, token| {
        let path = path.clone();
        async move {
            let resp = require_success(
                transport::settings_get(socket, &token, &path).await?,
                "get_model_device",
            )?;
            Ok(ModelDevice {
                device: resp.device.unwrap_or_default(),
                resolved_accel: resp.resolved_accel.flatten(),
                available_devices: resp.available_devices.unwrap_or_default(),
            })
        }
    })
    .await
}

/// Set a model's device (HTTP `POST /pipeline/{stage}/model/{model}/device`).
///
/// `device` is the `cpu`/`gpu` preference. Reloads the model when it is the one
/// its stage is running; otherwise only records the choice, so the next load
/// picks it up. A reload that fails puts the model back where it was and leaves
/// the setting alone — a choice is not recorded against a load that did not
/// happen.
pub async fn set_model_device(stage: u32, model: String, device: String) -> HttpResult<()> {
    let path = device_path(stage, &model);
    let body = serde_json::json!({ "device": device });
    with_settings_token(move |socket, token| {
        let path = path.clone();
        let body = body.clone();
        async move {
            let resp = transport::settings_post(socket, &token, &path, &body).await?;
            require_unit(resp, "set_model_device")
        }
    })
    .await
}

/// The devices a model can be loaded onto here
/// (HTTP `GET /pipeline/{stage}/model/{model}/device/list`).
///
/// The daemon's answer, not a client derivation: the model's declared devices
/// narrowed to the accelerators its installed asset ships and to what the host
/// actually has. A CUDA-only backend on a machine with no NVIDIA card installed
/// its CPU asset and is not offered a GPU here.
///
/// Empty for a model that synthesizes remotely, and for a local model this
/// install cannot run on anything. A picker hides itself on an empty list
/// rather than special-casing a status.
pub async fn list_model_devices(stage: u32, model: String) -> HttpResult<Vec<String>> {
    let path = format!("/pipeline/{stage}/model/{}/device/list", enc(&model));
    with_settings_token(move |socket, token| {
        let path = path.clone();
        async move {
            let resp = require_success(
                transport::settings_get(socket, &token, &path).await?,
                "list_model_devices",
            )?;
            Ok(resp.available_devices.unwrap_or_default())
        }
    })
    .await
}

/// The devices the stage's selected backend can run models on here
/// (HTTP `GET /pipeline/{stage}/device/list`) — the union of
/// [`list_model_devices`] over the models it serves in that stage's role.
///
/// The broader answer, for a control shown before any model is picked. Once one
/// is, [`list_model_devices`] is narrower and more accurate: a backend whose
/// large model needs a GPU still offers `cpu` here because another of its
/// models runs there.
pub async fn list_stage_devices(stage: u32) -> HttpResult<Vec<String>> {
    let path = format!("/pipeline/{stage}/device/list");
    with_settings_token(move |socket, token| {
        let path = path.clone();
        async move {
            let resp = require_success(
                transport::settings_get(socket, &token, &path).await?,
                "list_stage_devices",
            )?;
            Ok(resp.available_devices.unwrap_or_default())
        }
    })
    .await
}
