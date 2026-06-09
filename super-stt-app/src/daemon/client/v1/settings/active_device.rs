// SPDX-License-Identifier: GPL-3.0-only
//! `/active_device` — compute device (CPU/CUDA) + GPU memory.

use crate::daemon::client::internal::response::{require_success, require_unit};
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::transport;

/// `(free_bytes, total_bytes)` for a CUDA device; `None` on CPU or when the
/// driver couldn't be queried.
pub type GpuMemoryInfo = Option<(u64, u64)>;

/// Read current device + available devices + GPU memory (HTTP `GET /active_device`).
pub async fn get_current_device() -> Result<(String, Vec<String>, GpuMemoryInfo), String> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, "/active_device").await?,
            "get_device",
        )?;
        let device = resp.device.unwrap_or_else(|| "unknown".to_string());
        let available_devices = resp
            .available_devices
            .unwrap_or_else(|| vec!["cpu".to_string()]);
        let gpu_memory = match (resp.gpu_free_memory, resp.gpu_total_memory) {
            (Some(free), Some(total)) => Some((free, total)),
            _ => None,
        };
        Ok((device, available_devices, gpu_memory))
    })
    .await
}

/// Switch compute device (HTTP `POST /active_device`).
pub async fn set_device(device: String) -> Result<(), String> {
    with_settings_token(move |socket, token| {
        let device = device.clone();
        async move {
            let resp = transport::settings_post(
                socket,
                &token,
                "/active_device",
                &serde_json::json!({ "device": device }),
            )
            .await?;
            require_unit(resp, "set_device")
        }
    })
    .await
}
