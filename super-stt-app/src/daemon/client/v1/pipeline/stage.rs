// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline/{stage}` — the backend filling one stage.
//!
//! A transcript passes through ordered stages: stage 1 turns audio into text,
//! every later stage rewrites what the one before it produced. Every stage
//! answers this same path — it selects the backend filling the position and
//! reports which one that is — so there is one implementation here for all of
//! them. The model that backend runs is [`super::model`], one level down.
//!
//! It used to be one copy per stage, with the position baked into a `&str`
//! constant in each file. The copies drifted: stage 2's `set` kept the header
//! timeout that stage 1's documents skipping, which turned a first load's
//! download into a spurious failure. Taking the stage as a parameter is what
//! stops that happening again.

use serde::Deserialize;

use crate::daemon::client::internal::response::{require_success, require_unit};
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::http_client::transport;

/// The backend filling one stage, and whether the stage is switched on.
///
/// Only the fields the settings app consumes are modeled; serde ignores the
/// rest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct StageBackend {
    /// `None` when the stage is empty (no backend selected).
    #[serde(default)]
    pub source: Option<String>,
    /// The backend's display name; `None` when the stage is empty.
    #[serde(default)]
    pub name: Option<String>,
    /// Whether the user has this stage switched on — what Load sets and Unload
    /// clears.
    ///
    /// Every stage reports it. Stage 1 used to omit the field, and the app read
    /// the absence as "on exactly when a model is loaded", which is why an
    /// unload there emptied the card instead of leaving the selection to load
    /// again.
    #[serde(default)]
    pub enabled: bool,
}

/// The path a stage answers on. Its model answers one level down, in
/// [`super::model`].
fn stage_path(stage: u32) -> String {
    format!("/pipeline/{stage}")
}

/// Read `stage`'s backend (HTTP `GET /pipeline/{stage}`).
pub async fn get_stage(stage: u32) -> HttpResult<StageBackend> {
    with_settings_token(move |socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, &stage_path(stage)).await?,
            "get_pipeline_stage",
        )?;
        // A daemon that predates the pipeline omits the stage; read that as
        // "empty, nothing selected" rather than failing the settings load.
        Ok(resp
            .stage
            .map(|st| StageBackend {
                source: st.source,
                name: st.name,
                enabled: st.enabled,
            })
            .unwrap_or_default())
    })
    .await
}

/// Select the backend filling `stage` (HTTP `POST /pipeline/{stage}`).
///
/// Records which backend fills the stage and unloads a foreign model — it does
/// NOT load one. Pair with [`super::model::set_stage_model`] to also run a model.
pub async fn set_stage_backend(stage: u32, source: String) -> HttpResult<()> {
    with_settings_token(move |socket, token| {
        let source = source.clone();
        async move {
            let resp = transport::settings_post(
                socket,
                &token,
                &stage_path(stage),
                &serde_json::json!({ "source": source }),
            )
            .await?;
            require_unit(resp, "set_stage_backend")
        }
    })
    .await
}

/// Empty `stage`, forgetting the model with it
/// (HTTP `DELETE /pipeline/{stage}`).
///
/// This is a card's Deselect. [`super::model::unload_stage_model`] is the softer
/// one that keeps the backend *and* the model it was pointed at.
pub async fn clear_stage_backend(stage: u32) -> HttpResult<()> {
    with_settings_token(move |socket, token| async move {
        let resp = transport::settings_delete(socket, &token, &stage_path(stage)).await?;
        require_unit(resp, "clear_stage_backend")
    })
    .await
}
