// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline/{stage}/backend/list` — the backends that can fill a stage.
//!
//! The slot itself is [`super::stage`], one level up: it reports the backend
//! filling the position and selects one. This is the menu that selection
//! accepts.
//!
//! Asked rather than derived. The app holds the whole `/backend/list` catalog
//! and could filter it on each model\'s role — it did — but that is a second
//! implementation of the rule `POST /pipeline/{stage}` enforces, and a picker
//! built from a filter that drifts offers a backend the daemon then refuses.

use crate::daemon::client::internal::response::require_success;
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::http_client::transport;

use crate::daemon::backends::BackendInfo;

/// The backends that can fill `stage`, each carrying only the models it can run
/// there (HTTP `GET /pipeline/{stage}/backend/list`).
///
/// Narrowed on both axes by the daemon, so a picker renders straight from it:
/// which backends, and which of their models.
pub async fn list_stage_backends(stage: u32) -> HttpResult<Vec<BackendInfo>> {
    with_settings_token(move |socket, token| async move {
        let path = format!("/pipeline/{stage}/backend/list");
        let resp = require_success(
            transport::settings_get(socket, &token, &path).await?,
            "list_stage_backends",
        )?;
        Ok(resp
            .backends
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default())
    })
    .await
}
