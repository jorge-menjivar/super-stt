// SPDX-License-Identifier: GPL-3.0-only
//! `/active_model` and `/models` — model selection, download, and reload.

use crate::daemon::client::internal::response::{require_message, require_success};
use crate::daemon::client::internal::session::with_settings_token;
use serde::Deserialize;
use super_stt_shared::daemon::http_client::transport;

// Only the `/active_model` fields the settings app consumes are modeled; serde ignores the rest.

/// Wire shape returned by `GET /active_model`.
#[derive(Debug, Clone, Deserialize)]
pub struct ActiveModelStatus {
    pub active_model: ActiveModelPayload,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActiveModelPayload {
    pub current: ActiveModelCurrent,
    pub switch: Option<ActiveModelSwitch>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActiveModelCurrent {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActiveModelSwitch {
    pub phase: String,
    pub target: serde_json::Value,
    pub started_at: Option<String>,
    pub download: Option<ActiveModelDownload>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActiveModelDownload {
    pub current_file: String,
    pub file_index: usize,
    pub total_files: usize,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub percentage: f32,
    pub eta_seconds: Option<u64>,
}

async fn fetch_active_model(
    socket: std::path::PathBuf,
    token: &str,
) -> Result<ActiveModelStatus, String> {
    transport::get_json::<ActiveModelStatus>(socket, token, "/active_model")
        .await
        .map_err(String::from)
}

/// Get current loaded model from daemon as `(name, provider, source)`
/// (HTTP `GET /active_model`).
pub async fn get_current_model()
-> Result<(String, super_stt_shared::models::provider::Provider, String), String> {
    with_settings_token(|socket, token| async move {
        let status = fetch_active_model(socket, &token).await?;
        let current = status.active_model.current;
        // Idle daemon (no model loaded) is a valid state: report an empty
        // selection rather than erroring, so the UI shows nothing selected.
        let Some(model) = current.model else {
            return Ok((
                String::new(),
                super_stt_shared::models::provider::Provider::default(),
                String::new(),
            ));
        };
        let provider_str = current
            .provider
            .ok_or("missing active_model.current.provider")?;
        let provider: super_stt_shared::models::provider::Provider = provider_str
            .parse()
            .map_err(|e| format!("invalid provider {provider_str:?}: {e}"))?;
        let source = current.source.unwrap_or_default();
        Ok((model, provider, source))
    })
    .await
}

/// Get current download status. Composed from `/active_model`'s
/// `switch.download` sub-object.
pub async fn get_download_status()
-> Result<Option<super_stt_shared::models::protocol::DownloadProgress>, String> {
    with_settings_token(|socket, token| async move {
        let status = fetch_active_model(socket, &token).await?;
        let Some(switch) = status.active_model.switch else {
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
        let progress = super_stt_shared::models::protocol::DownloadProgress {
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
        };
        Ok(Some(progress))
    })
    .await
}

/// Set/switch to a different model (HTTP `POST /active_model`).
pub async fn set_model(
    model: String,
    provider: super_stt_shared::models::provider::Provider,
    source: String,
) -> Result<String, String> {
    let provider_str = provider.to_string();
    let source_str = source;
    with_settings_token(move |socket, token| {
        let model = model.clone();
        let provider_str = provider_str.clone();
        let source_str = source_str.clone();
        async move {
            let mut body = serde_json::json!({ "model": model, "provider": provider_str });
            body["source"] = serde_json::Value::String(source_str);
            let resp = transport::settings_post(socket, &token, "/active_model", &body).await?;
            require_message(resp, "set_model")
        }
    })
    .await
}

/// List all available models from daemon (HTTP `GET /models`).
pub async fn list_available_models()
-> Result<Vec<(String, super_stt_shared::models::provider::Provider, String)>, String> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, "/models").await?,
            "list_models",
        )?;
        Ok(resp.available_models.unwrap_or_default())
    })
    .await
}

/// Cancel any ongoing model-switch download (HTTP `POST /active_model/cancel`).
pub async fn cancel_download() -> Result<String, String> {
    with_settings_token(|socket, token| async move {
        let resp = transport::settings_post(
            socket,
            &token,
            "/active_model/cancel",
            &serde_json::json!({}),
        )
        .await?;
        require_message(resp, "cancel_download")
    })
    .await
}

/// Reload the active model (HTTP `POST /active_model/reload`) so a changed
/// secret/option for its backend takes effect immediately.
pub async fn reload_active_model() -> Result<String, String> {
    with_settings_token(|socket, token| async move {
        let resp = transport::settings_post(
            socket,
            &token,
            "/active_model/reload",
            &serde_json::json!({}),
        )
        .await?;
        require_message(resp, "reload_active_model")
    })
    .await
}

/// Unload the currently loaded model (HTTP `DELETE /active_model`). The
/// active backend stays selected; the user can then pick another of its
/// models. Use [`clear_active_backend`] to return the daemon to idle.
pub async fn unload_active_model() -> Result<String, String> {
    with_settings_token(|socket, token| async move {
        let resp = transport::settings_delete(socket, &token, "/active_model").await?;
        require_message(resp, "unload_active_model")
    })
    .await
}
