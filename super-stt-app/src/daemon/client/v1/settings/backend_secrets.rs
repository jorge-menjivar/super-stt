// SPDX-License-Identifier: GPL-3.0-only
//! `/backends/{source}/secrets/…` — per-backend secret management.

use crate::daemon::client::internal::response::{require_success, require_unit};
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::transport;

fn enc(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

/// Store a backend secret via the daemon (HTTP `POST /backends/{source}/secrets/{name}`).
pub async fn set_backend_secret(source: String, name: String, value: String) -> Result<(), String> {
    with_settings_token(move |socket, token| {
        let (source, name, value) = (source.clone(), name.clone(), value.clone());
        async move {
            let path = format!("/backends/{}/secrets/{}", enc(&source), enc(&name));
            let resp = transport::settings_post(
                socket,
                &token,
                &path,
                &serde_json::json!({ "value": value }),
            )
            .await?;
            require_unit(resp, "set_backend_secret")
        }
    })
    .await
}

/// Clear a backend secret via the daemon (HTTP `DELETE /backends/{source}/secrets/{name}`).
pub async fn clear_backend_secret(source: String, name: String) -> Result<(), String> {
    with_settings_token(move |socket, token| {
        let (source, name) = (source.clone(), name.clone());
        async move {
            let path = format!("/backends/{}/secrets/{}", enc(&source), enc(&name));
            let resp = transport::settings_delete(socket, &token, &path).await?;
            require_unit(resp, "clear_backend_secret")
        }
    })
    .await
}

/// List which secrets are configured for a backend (HTTP `GET /backends/{source}/secrets/list`).
/// Returns a `Vec<(name, configured)>` pair for each declared secret.
pub async fn list_backend_secrets(source: String) -> Result<Vec<(String, bool)>, String> {
    with_settings_token(move |socket, token| {
        let source = source.clone();
        async move {
            let path = format!("/backends/{}/secrets/list", enc(&source));
            let resp = require_success(
                transport::settings_get(socket, &token, &path).await?,
                "list_backend_secrets",
            )?;
            let arr = resp.secrets.unwrap_or(serde_json::Value::Array(vec![]));
            let parsed: Vec<serde_json::Value> = serde_json::from_value(arr).unwrap_or_default();
            Ok(parsed
                .into_iter()
                .filter_map(|v| {
                    Some((
                        v.get("name")?.as_str()?.to_string(),
                        v.get("configured")?.as_bool()?,
                    ))
                })
                .collect())
        }
    })
    .await
}
