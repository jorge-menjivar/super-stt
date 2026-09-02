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
    /// The accelerator the loaded model actually runs on (`cpu`, `cuda`, …);
    /// `None` until one has loaded.
    #[serde(default)]
    pub device: Option<String>,
    /// The stage's own `cpu`/`gpu` preference; `None` when it has none and
    /// follows the transcription stage's. Absent from a daemon that predates
    /// per-stage devices, which reads the same way.
    #[serde(default)]
    pub preferred_device: Option<String>,
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

/// Run final transcripts through `model` (HTTP `POST /pipeline/2/model`).
///
/// `source` is optional: omitted, the daemon resolves the model against the
/// selected post-processor backend, exactly as `POST /active_model` resolves
/// against the active one. `device` is the stage's own `cpu`/`gpu`; omitted,
/// the daemon keeps the one it has stored.
pub async fn set_post_processor(
    model: String,
    source: Option<String>,
    device: Option<String>,
) -> HttpResult<()> {
    let mut body = serde_json::json!({ "model": model });
    if let Some(source) = source {
        body["source"] = serde_json::Value::String(source);
    }
    if let Some(device) = device {
        body["device"] = serde_json::Value::String(device);
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

#[cfg(test)]
mod tests {
    use super::PostProcessorState;

    /// A daemon that predates per-stage devices sends neither field; the
    /// state reads as "no preference, nothing resolved" rather than failing
    /// the settings load.
    #[test]
    fn a_stage_without_device_fields_still_parses() {
        let state: PostProcessorState = serde_json::from_value(serde_json::json!({
            "stage": 2, "role": "post_processor",
            "source": "github.com/x/y", "model": "cleanup", "loaded": true, "enabled": true
        }))
        .expect("older payload must parse");
        assert_eq!(state.device, None);
        assert_eq!(state.preferred_device, None);
        assert_eq!(
            state.selection(),
            Some(("cleanup".to_string(), "github.com/x/y".to_string()))
        );
    }

    /// The ask and the answer are separate fields and both come through.
    #[test]
    fn the_stages_device_fields_carry_through() {
        let state: PostProcessorState = serde_json::from_value(serde_json::json!({
            "model": "cleanup", "source": "github.com/x/y",
            "loaded": true, "enabled": true,
            "device": "cuda", "preferred_device": "gpu"
        }))
        .expect("parses");
        assert_eq!(state.device.as_deref(), Some("cuda"));
        assert_eq!(state.preferred_device.as_deref(), Some("gpu"));
    }
}
