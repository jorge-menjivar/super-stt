// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline/{stage}/model/{model}/device` — the device a model runs on.
//!
//! A device belongs to a model, and is addressed through the stage that runs
//! it: the model resolves against that stage's selected backend, and a change
//! for the model loaded there is a reload, while for any other it is a note
//! for the next load. That last case is what lets the card set the device
//! *before* Load, then load, without the daemon reloading twice.
//!
//! GPU memory is served separately by `GET /gpu_info` (gpu-probe).

use serde::Deserialize;

use crate::daemon::client::internal::response::{require_success, require_unit};
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::http_client::transport;

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
    format!("/pipeline/{stage}/model/{model}/device")
}

/// Read a model's device (HTTP `GET /pipeline/{stage}/model/{model}/device`).
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
/// `device` is the `cpu`/`gpu` preference. Reloads the model when it is the
/// one its stage is running; otherwise only records the choice.
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
