// SPDX-License-Identifier: GPL-3.0-only
//! `/backends`, `/active_backend`, `/gpu_info` — backend catalog and selection.

use crate::daemon::backends::BackendInfo;
use crate::daemon::client::internal::response::{require_message, require_success, require_unit};
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::transport;

/// List installed backends with the models, secrets, and options they
/// declare (HTTP `GET /backends`). An empty or absent catalog yields an
/// empty `Vec`.
pub async fn list_backends() -> Result<Vec<BackendInfo>, String> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, "/backends").await?,
            "list_backends",
        )?;
        match resp.backends {
            Some(value) => {
                serde_json::from_value(value).map_err(|e| format!("failed to parse backends: {e}"))
            }
            None => Ok(Vec::new()),
        }
    })
    .await
}

/// Set or clear a backend option (HTTP `POST /backends/option`). An
/// empty `value` clears the override and reverts to the default.
pub async fn set_backend_option(
    source: String,
    name: String,
    value: String,
) -> Result<String, String> {
    with_settings_token(move |socket, token| {
        let source = source.clone();
        let name = name.clone();
        let value = value.clone();
        async move {
            let resp = transport::settings_post(
                socket,
                &token,
                "/backends/option",
                &serde_json::json!({ "source": source, "name": name, "value": value }),
            )
            .await?;
            require_message(resp, "set_backend_option")
        }
    })
    .await
}

/// Get the active backend's `source` (HTTP `GET /active_backend`). `None` when
/// the daemon is idle (no backend selected).
pub async fn get_active_backend() -> Result<Option<String>, String> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, "/active_backend").await?,
            "get_active_backend",
        )?;
        Ok(resp
            .active_backend
            .as_ref()
            .and_then(|v| v.get("source"))
            .and_then(serde_json::Value::as_str)
            .map(String::from))
    })
    .await
}

/// Select the active backend by `source` (HTTP `POST /active_backend`).
/// Records which backend is active and unloads a foreign model — does NOT
/// load a model. Pair with `set_model` to also load one.
pub async fn set_active_backend(source: String) -> Result<(), String> {
    with_settings_token(move |socket, token| {
        let source = source.clone();
        async move {
            let resp = transport::settings_post(
                socket,
                &token,
                "/active_backend",
                &serde_json::json!({ "source": source }),
            )
            .await?;
            require_unit(resp, "set_active_backend")
        }
    })
    .await
}

/// Deselect the active backend (HTTP `DELETE /active_backend`) → daemon idle.
pub async fn clear_active_backend() -> Result<(), String> {
    with_settings_token(|socket, token| async move {
        let resp = transport::settings_delete(socket, &token, "/active_backend").await?;
        require_unit(resp, "clear_active_backend")
    })
    .await
}

/// GPU inventory + memory (HTTP `GET /gpu_info`). Empty when no GPU is detected.
pub async fn get_gpu_info() -> Result<Vec<super_stt_shared::models::protocol::GpuInfo>, String> {
    use super_stt_shared::models::protocol::GpuInfo;
    log::debug!("get_gpu_info: requesting GET /gpu_info");
    let result = with_settings_token(|socket, token| async move {
        let resp = transport::settings_get(socket, &token, "/gpu_info").await?;
        log::debug!(
            "get_gpu_info: response status={:?} gpu_info={:?}",
            resp.status,
            resp.gpu_info
        );
        let resp = require_success(resp, "get_gpu_info")?;
        match resp.gpu_info {
            Some(v) => serde_json::from_value::<Vec<GpuInfo>>(v)
                .map_err(|e| format!("parse gpu_info: {e}")),
            None => Ok(Vec::new()),
        }
    })
    .await;
    match &result {
        Ok(gpus) => log::debug!("get_gpu_info: parsed {} GPU(s): {gpus:?}", gpus.len()),
        Err(e) => log::warn!("get_gpu_info: request failed: {e}"),
    }
    result
}
