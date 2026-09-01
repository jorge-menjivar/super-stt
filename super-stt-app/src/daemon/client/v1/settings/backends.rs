// SPDX-License-Identifier: GPL-3.0-only
//! `/backends`, `/active_backend`, `/gpu_info` — backend catalog and selection.

use crate::daemon::backends::BackendInfo;
use crate::daemon::client::internal::response::{require_success, require_unit};
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::transport;

/// Transcription is stage 1 of the pipeline.
const STT_STAGE: &str = "/pipeline/1";
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

/// Get the backend filling stage 1 (HTTP `GET /pipeline/1`). `None` when the
/// stage is empty (no backend selected).
pub async fn get_active_backend() -> HttpResult<Option<String>> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, STT_STAGE).await?,
            "get_pipeline_stage",
        )?;
        Ok(resp
            .stage
            .as_ref()
            .and_then(|v| v.get("source"))
            .and_then(serde_json::Value::as_str)
            .map(String::from))
    })
    .await
}

/// Select the backend filling stage 1 (HTTP `POST /pipeline/1`). Records which
/// backend transcribes and unloads a foreign model — does NOT load one. Pair
/// with `set_model` to also load a model.
pub async fn set_active_backend(source: String) -> HttpResult<()> {
    with_settings_token(move |socket, token| {
        let source = source.clone();
        async move {
            let resp = transport::settings_post(
                socket,
                &token,
                STT_STAGE,
                &serde_json::json!({ "source": source }),
            )
            .await?;
            require_unit(resp, "set_stage_backend")
        }
    })
    .await
}

/// Empty stage 1 (HTTP `DELETE /pipeline/1`) → daemon idle.
pub async fn clear_active_backend() -> HttpResult<()> {
    with_settings_token(|socket, token| async move {
        let resp = transport::settings_delete(socket, &token, STT_STAGE).await?;
        require_unit(resp, "clear_stage_backend")
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
