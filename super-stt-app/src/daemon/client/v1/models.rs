// SPDX-License-Identifier: GPL-3.0-only
//! `/models` — the flat model catalog.
//!
//! Every model the installed backends serve, with no stage in it: which of them
//! a *stage* can run is decided by role, through `roles::models_for`. The full
//! per-backend catalog, roles included, is [`super::backends::list_backends`].

use crate::daemon::client::internal::response::require_success;
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::http_client::transport;

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
