// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline/{stage}` — the backend filling one stage.
//!
//! Only the backend. What it is running is [`super::model`], one level down the
//! path. The pair is deliberate: a card's Select and its Load are separate
//! acts, and [`clear_stage_backend`] here forgets the backend where
//! [`super::model::unload_stage_model`] keeps it, so a user who only wanted
//! their VRAM back does not lose the choice they made.
//!
//! This replaces `/active_backend`, which named the selection rather than the
//! position holding it. Taking the stage as a parameter is what lets a second
//! position — a text normalizer ahead of synthesis, say — reuse this code
//! instead of growing a parallel copy that drifts from it.

use serde::Deserialize;

use crate::daemon::client::internal::response::require_unit;
use crate::daemon::client::internal::session::with_settings_token;
use super_tts_shared::daemon::http_client::HttpResult;
use super_tts_shared::daemon::http_client::transport;

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
    /// Whether the user has this stage switched on — what Load sets and
    /// Deselect clears.
    ///
    /// Distinct from whether the model came up, which is `loaded` on
    /// [`super::model::StageModel`]. Reading this as "a model is running" is
    /// what would make a card go quiet about a load that failed.
    #[serde(default)]
    pub enabled: bool,
}

/// Wire envelope for `GET /pipeline/{stage}`.
///
/// Parsed here rather than off the shared `DaemonResponse`, which carries the
/// whole pipeline under `pipeline` and has no field for one stage at all — so
/// reading this endpoint that way would yield `None` on every call, with no
/// error and no log line, and the Models card would come up empty on a daemon
/// that had faithfully remembered the selection.
#[derive(Debug, Clone, Deserialize)]
struct StageEnvelope {
    #[serde(default)]
    status: String,
    #[serde(default)]
    stage: StageBackend,
}

/// The path a stage answers on. Its model answers one level down, in
/// [`super::model`].
fn stage_path(stage: u32) -> String {
    format!("/pipeline/{stage}")
}

/// Read `stage`'s backend (HTTP `GET /pipeline/{stage}`).
pub async fn get_stage(stage: u32) -> HttpResult<StageBackend> {
    with_settings_token(move |socket, token| async move {
        let envelope =
            transport::get_json::<StageEnvelope>(socket, &token, &stage_path(stage)).await?;
        if envelope.status != "success" {
            return Err(super_tts_shared::daemon::http_client::HttpError::Other(
                format!("get_stage: daemon answered {}", envelope.status),
            ));
        }
        Ok(envelope.stage)
    })
    .await
}

/// Select the backend filling `stage` (HTTP `POST /pipeline/{stage}`).
///
/// Records which backend fills the stage and unloads a foreign model — it does
/// NOT load one, and cannot fail for the runtime reasons a load can. Pair with
/// [`super::model::set_stage_model`] to also run a model.
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
/// This is a card's Deselect, and emptying stage 1 returns the daemon to idle —
/// `POST /speak` answers `409 model_not_loaded` until something is chosen
/// again. [`super::model::unload_stage_model`] is the softer one that frees the
/// device memory and keeps both the backend and the model it was pointed at.
pub async fn clear_stage_backend(stage: u32) -> HttpResult<()> {
    with_settings_token(move |socket, token| async move {
        let resp = transport::settings_delete(socket, &token, &stage_path(stage)).await?;
        require_unit(resp, "clear_stage_backend")
    })
    .await
}
