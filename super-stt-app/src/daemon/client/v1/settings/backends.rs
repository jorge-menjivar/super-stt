// SPDX-License-Identifier: GPL-3.0-only
//! `/backends`, `/models`, `/gpu_info` — the backend and model catalog.
//!
//! Filling a *stage* with one of these backends is `settings::stage`'s job.

use crate::daemon::backends::BackendInfo;
use crate::daemon::client::internal::response::{require_success, require_unit};
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::transport;

use super_stt_shared::daemon::http_client::{HttpError, HttpResult};

/// List installed backends with the models, secrets, and options they
/// declare (HTTP `GET /backends`). An empty or absent catalog yields an
/// empty `Vec`.
pub async fn list_backends() -> HttpResult<Vec<BackendInfo>> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, "/backends").await?,
            "list_backends",
        )?;
        match resp.backends {
            Some(value) => serde_json::from_value(value)
                .map_err(|e| HttpError::Other(format!("failed to parse backends: {e}"))),
            None => Ok(Vec::new()),
        }
    })
    .await
}

/// Set a backend option (HTTP `POST /backends/{source}/options/{name}`).
pub async fn set_backend_option(source: String, name: String, value: String) -> HttpResult<()> {
    with_settings_token(move |socket, token| {
        let (source, name, value) = (source.clone(), name.clone(), value.clone());
        async move {
            let path = format!(
                "/backends/{}/options/{}",
                urlencoding::encode(&source),
                urlencoding::encode(&name)
            );
            let resp = transport::settings_post(
                socket,
                &token,
                &path,
                &serde_json::json!({ "value": value }),
            )
            .await?;
            require_unit(resp, "set_backend_option")
        }
    })
    .await
}

/// Clear a backend option (HTTP `DELETE /backends/{source}/options/{name}`).
pub async fn clear_backend_option(source: String, name: String) -> HttpResult<()> {
    with_settings_token(move |socket, token| {
        let (source, name) = (source.clone(), name.clone());
        async move {
            let path = format!(
                "/backends/{}/options/{}",
                urlencoding::encode(&source),
                urlencoding::encode(&name)
            );
            let resp = transport::settings_delete(socket, &token, &path).await?;
            require_unit(resp, "clear_backend_option")
        }
    })
    .await
}

/// List all available models from daemon (HTTP `GET /models`).
///
/// The flat catalog, with no stage in it: which models a *stage* can run is
/// decided by role, through `roles::models_for`.
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
