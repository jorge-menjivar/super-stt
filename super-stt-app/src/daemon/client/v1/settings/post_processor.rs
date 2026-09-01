// SPDX-License-Identifier: GPL-3.0-only
//! Stage 2 of the pipeline — the post-processor that rewrites final
//! transcripts.
//!
//! A transcript passes through ordered stages: stage 1 turns audio into text,
//! stage 2 rewrites it. Both are addressed the same way — `/pipeline/{stage}`
//! selects the backend filling a stage, `/pipeline/{stage}/model` runs a model
//! in it — so these wrappers are stage 2 of a shape that also serves stage 1
//! and any stage a future chain adds.
//!
//! Hand-written rather than macro-generated: the payload is an object, which
//! the flat `settings_getter!`/`settings_setter!` shapes cannot express.

use serde::Deserialize;

use crate::daemon::client::internal::response::{require_success, require_unit};
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::http_client::transport;

/// Post-processing is stage 2 of the pipeline. Named here so the position
/// appears once, and a re-ordering is a one-line change.
const PP_STAGE: &str = "/pipeline/2";
const PP_STAGE_MODEL: &str = "/pipeline/2/model";

/// The daemon's `post_processor` block.
///
/// `enabled` and `loaded` are distinct: a selection can be enabled while its
/// model failed to load, in which case transcripts pass through unprocessed —
/// which is what the UI shows the user.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct PostProcessorState {
    #[serde(default)]
    pub enabled: bool,
    /// `None` when nothing is selected.
    #[serde(default)]
    pub model: Option<String>,
    /// Repo id of the backend serving `model`; `None` when nothing is selected.
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub loaded: bool,
}

impl PostProcessorState {
    /// The selected `(model, source)` pair, when the selection is complete.
    #[must_use]
    pub fn selection(&self) -> Option<(String, String)> {
        Some((self.model.clone()?, self.source.clone()?))
    }
}

/// Read stage 2's state (HTTP `GET /pipeline/2`).
pub async fn get_post_processor() -> HttpResult<PostProcessorState> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, PP_STAGE).await?,
            "get_pipeline_stage",
        )?;
        // A daemon that predates the pipeline omits the stage; read that as
        // "off, nothing selected" rather than failing the settings load.
        Ok(resp
            .stage
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default())
    })
    .await
}

/// Stop running the post-processor, keeping the backend selected
/// (HTTP `DELETE /post_processor`).
///
/// This is the card's Disable, the analogue of unloading a model: the choice
/// survives so re-enabling is one click. [`clear_post_processor_backend`] is
/// the one that forgets it.
pub async fn clear_post_processor() -> HttpResult<()> {
    with_settings_token(|socket, token| async move {
        let resp = transport::settings_delete(socket, &token, PP_STAGE_MODEL).await?;
        require_unit(resp, "clear_stage_model")
    })
    .await
}

/// Run final transcripts through `model` (HTTP `POST /post_processor`).
///
/// `source` is optional: omitted, the daemon resolves the model against the
/// selected post-processor backend, exactly as `POST /active_model` resolves
/// against the active one.
pub async fn set_post_processor(model: String, source: Option<String>) -> HttpResult<()> {
    let mut body = serde_json::json!({ "model": model });
    if let Some(source) = source {
        body["source"] = serde_json::Value::String(source);
    }
    with_settings_token(move |socket, token| {
        let body = body.clone();
        async move {
            let resp = transport::settings_post(socket, &token, PP_STAGE_MODEL, &body).await?;
            require_unit(resp, "set_stage_model")
        }
    })
    .await
}

/// Select the backend that fills stage 2 (HTTP `POST /pipeline/2`).
pub async fn set_post_processor_backend(source: String) -> HttpResult<()> {
    let body = serde_json::json!({ "source": source });
    with_settings_token(move |socket, token| {
        let body = body.clone();
        async move {
            let resp = transport::settings_post(socket, &token, PP_STAGE, &body).await?;
            require_unit(resp, "set_stage_backend")
        }
    })
    .await
}

/// Deselect stage 2's backend, forgetting the model with it
/// (HTTP `DELETE /pipeline/2`).
///
/// This is the card's Deselect, the analogue of clearing the active backend.
pub async fn clear_post_processor_backend() -> HttpResult<()> {
    with_settings_token(|socket, token| async move {
        let resp = transport::settings_delete(socket, &token, PP_STAGE).await?;
        require_unit(resp, "clear_stage_backend")
    })
    .await
}
