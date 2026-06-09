// SPDX-License-Identifier: GPL-3.0-only
//! `/write_method` — text-output strategy (auto, xdotool, clipboard, …).

use crate::daemon::client::internal::response::{require_success, require_unit};
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::transport;

/// Get write method (HTTP `GET /write_method`).
pub async fn get_write_method() -> Result<String, String> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, "/write_method").await?,
            "get_write_method",
        )?;
        Ok(resp.write_method.unwrap_or_else(|| "auto".to_string()))
    })
    .await
}

/// Set write method (HTTP `POST /write_method`).
pub async fn set_write_method(method: String) -> Result<(), String> {
    with_settings_token(move |socket, token| {
        let method = method.clone();
        async move {
            let resp = transport::settings_post(
                socket,
                &token,
                "/write_method",
                &serde_json::json!({ "method": method }),
            )
            .await?;
            require_unit(resp, "set_write_method")
        }
    })
    .await
}
