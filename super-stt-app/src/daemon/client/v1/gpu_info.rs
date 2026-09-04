// SPDX-License-Identifier: GPL-3.0-only
//! `/gpu_info` — the host's GPUs.
//!
//! The hardware inventory, independent of any model. What a *particular* model
//! can run on here is narrower, and is
//! [`super::pipeline::device::list_model_devices`].

use crate::daemon::client::internal::response::require_success;
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::http_client::transport;

/// GPU inventory + memory (HTTP `GET /gpu_info`). Empty when no GPU is detected.
pub async fn get_gpu_info() -> HttpResult<Vec<super_stt_shared::models::protocol::GpuInfo>> {
    log::debug!("get_gpu_info: requesting GET /gpu_info");
    let result = with_settings_token(|socket, token| async move {
        let resp = transport::settings_get(socket, &token, "/gpu_info").await?;
        log::debug!(
            "get_gpu_info: response status={:?} gpu_info={:?}",
            resp.status,
            resp.gpu_info
        );
        let resp = require_success(resp, "get_gpu_info")?;
        // The field is already typed `Option<Vec<GpuInfo>>` on the wire, so no
        // second parse is needed — absent means no GPUs.
        Ok(resp.gpu_info.unwrap_or_default())
    })
    .await;
    match &result {
        Ok(gpus) => log::debug!("get_gpu_info: parsed {} GPU(s): {gpus:?}", gpus.len()),
        Err(e) => log::warn!("get_gpu_info: request failed: {e}"),
    }
    result
}
