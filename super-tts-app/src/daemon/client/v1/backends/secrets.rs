// SPDX-License-Identifier: GPL-3.0-only
//! `/backend/{source}/secret/…` — a backend's credentials.
//!
//! The mirror image of [`super::options`], with one difference that shapes
//! every call here: a value goes in and never comes back. There is no read
//! endpoint for a stored credential, only one that says whether *a* value is
//! stored, so a Configure sheet can render a filled field without ever having
//! seen what is in it.
//!
//! These paths carry the daemon's `secrets` scope rather than `settings`. The
//! app's cached token already asks for both, so nothing extra happens here —
//! but a token that carried only `settings` would get a `403` from these three
//! and from nothing else in this tree.

use crate::daemon::client::internal::response::{require_success, require_unit};
use crate::daemon::client::internal::session::with_settings_token;
use super_tts_shared::daemon::http_client::HttpResult;
use super_tts_shared::daemon::http_client::transport;

fn enc(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

/// Write a credential to the system keyring
/// (HTTP `POST /backend/{source}/secret/{name}`).
///
/// Replaces whatever was there. If the pipeline is running a model from this
/// backend the daemon reloads it, so a rotated key takes effect without the
/// user knowing a reload was needed.
pub async fn set_backend_secret(source: String, name: String, value: String) -> HttpResult<()> {
    with_settings_token(move |socket, token| {
        let (source, name, value) = (source.clone(), name.clone(), value.clone());
        async move {
            let path = format!("/backend/{}/secret/{}", enc(&source), enc(&name));
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

/// Remove a stored credential
/// (HTTP `DELETE /backend/{source}/secret/{name}`).
///
/// The value leaves the keyring rather than being ignored — this is what a
/// "disconnect account" control has to call for the promise it makes to be
/// true.
pub async fn clear_backend_secret(source: String, name: String) -> HttpResult<()> {
    with_settings_token(move |socket, token| {
        let (source, name) = (source.clone(), name.clone());
        async move {
            let path = format!("/backend/{}/secret/{}", enc(&source), enc(&name));
            let resp = transport::settings_delete(socket, &token, &path).await?;
            require_unit(resp, "clear_backend_secret")
        }
    })
    .await
}

/// Which of a backend's declared secrets are set
/// (HTTP `GET /backend/{source}/secret/list`).
///
/// Returns `(name, configured)` per declared secret. The value is never in the
/// answer — `configured` is the whole of what the daemon will say — so a form
/// renders a placeholder for a set secret rather than its contents.
///
/// The entries are picked out of the array by name rather than deserialized
/// into a struct: the daemon sends `label` and `required` alongside, and a
/// field added there should not stop the two the app reads from arriving.
pub async fn list_backend_secrets(source: String) -> HttpResult<Vec<(String, bool)>> {
    with_settings_token(move |socket, token| {
        let source = source.clone();
        async move {
            let path = format!("/backend/{}/secret/list", enc(&source));
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
