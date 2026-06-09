// SPDX-License-Identifier: GPL-3.0-only
//! `/custom_models_dir` — optional override for where local models are stored.

use crate::daemon::client::internal::response::{require_success, require_unit};
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::transport;

/// Read configured custom-models directory (HTTP `GET /custom_models_dir`).
pub async fn get_custom_models_dir() -> Result<Option<String>, String> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, "/custom_models_dir").await?,
            "get_custom_models_dir",
        )?;
        Ok(resp.custom_models_dir.unwrap_or(None))
    })
    .await
}

/// Set custom-models directory (HTTP `POST /custom_models_dir`).
pub async fn set_custom_models_dir(path: Option<String>) -> Result<(), String> {
    with_settings_token(move |socket, token| {
        let path = path.clone();
        async move {
            let resp = transport::settings_post(
                socket,
                &token,
                "/custom_models_dir",
                &serde_json::json!({ "path": path }),
            )
            .await?;
            require_unit(resp, "set_custom_models_dir")
        }
    })
    .await
}
