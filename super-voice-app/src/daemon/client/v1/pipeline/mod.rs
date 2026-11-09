// SPDX-License-Identifier: GPL-3.0-only
//! `/stt/pipeline` — the ordered stages a transcript passes through.
//!
//! Mirrors the daemon's `v1/pipeline/` tree: [`stage`] wraps `/stt/pipeline/{stage}`,
//! [`backend`] the menu that fills it, [`model`] wraps
//! `/stt/pipeline/{stage}/model` and its verbs, [`device`] and [`language`] wrap the
//! two per-model preferences.
//!
//! [`StageState`] sits here, above them, because it is the one thing that
//! belongs to no single path: a card draws a stage's backend and its model
//! together, and those are two endpoints. They are two endpoints because they
//! are two different things — a backend selection that outlives everything, and
//! a model that has a runtime — and joining them in the client is cheaper than
//! the drift that came of joining them on the wire.

pub(crate) mod backend;
pub(crate) mod device;
pub(crate) mod language;
pub(crate) mod model;
pub(crate) mod stage;
pub(crate) mod voice;

use crate::state::Stage;
use super_voice_shared::daemon::http_client::HttpResult;

pub use model::StageDevice;

/// One pipeline stage as a card renders it.
///
/// The union of [`stage::StageBackend`] and [`model::StageModel`], fetched by
/// [`get_stage_view`]. Keeping the two halves distinct in the types would push
/// the join into every view; keeping them distinct on the wire is what stops
/// the stages behaving differently, and that is the part that mattered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StageState {
    /// `None` when the stage is empty (no backend selected).
    pub source: Option<String>,
    /// The backend's display name; `None` when the stage is empty.
    pub name: Option<String>,
    /// Whether the user has this stage switched on.
    ///
    /// Not the same as [`Self::loaded`]: a stage can be switched on while its
    /// model failed to come up, and the card says so rather than silently
    /// looking idle.
    pub enabled: bool,
    /// The model the stage is pointed at; `None` when none is picked. Survives
    /// an unload.
    pub model: Option<String>,
    /// Whether that model is up.
    pub loaded: bool,
    /// The device the selection runs on; `None` when nothing is selected.
    ///
    /// Read through [`Self::running_device`]. The value a device *picker*
    /// shows comes from `GET /stt/pipeline/{stage}/model/{model}/device` instead,
    /// alongside the list it offers — one answer for one control, rather than
    /// the preference arriving by one route and the options by another.
    pub device: Option<StageDevice>,
}

impl StageState {
    /// The selected `(model, source)` pair, when the selection is complete.
    #[must_use]
    pub fn selection(&self) -> Option<(String, String)> {
        Some((self.model.clone()?, self.source.clone()?))
    }

    /// The accelerator the model is actually running on, or `None` when it is
    /// not running.
    #[must_use]
    pub fn running_device(&self) -> Option<&str> {
        if !self.loaded {
            return None;
        }
        self.device
            .as_ref()
            .and_then(|d| d.resolved_accel.as_deref())
            .filter(|d| !d.is_empty() && *d != "none")
    }
}

/// Read a whole stage: its backend and its model slot
/// (HTTP `GET /stt/pipeline/{stage}` then `GET /stt/pipeline/{stage}/model`).
///
/// Two requests over a Unix socket, which is what a card needs and what the
/// split costs. A failure in either fails the read, since a half-drawn card is
/// worse than one that reports it could not load.
pub async fn get_stage_view(stage: Stage) -> HttpResult<StageState> {
    let backend = stage::get_stage(stage).await?;
    let model = model::get_stage_model(stage).await?;
    Ok(StageState {
        source: backend.source,
        name: backend.name,
        enabled: switched_on(stage, backend.enabled, model.loaded),
        model: model.model,
        loaded: model.loaded,
        device: model.device,
    })
}

/// Whether `stage` is switched on, from what the daemon reports of it.
///
/// Post-processing has a switch of its own, which Load sets and Unload clears
/// whether or not the model comes up, so `enabled` with nothing loaded is a
/// stage meant to run that does not. Synthesis has none: the daemon reports it
/// on exactly when a backend is selected, the only state text to speech keeps.
/// Taken as it is, a backend picked a moment ago read as a model that failed to
/// come up, and its card offered Unload, which changes nothing, instead of a
/// model to load. It is on when its model runs.
fn switched_on(stage: Stage, reported: bool, loaded: bool) -> bool {
    match stage {
        Stage::Synthesis => reported && loaded,
        Stage::Transcription | Stage::PostProcessing => reported,
    }
}

#[cfg(test)]
mod switch_tests {
    use super::*;

    /// A synthesis backend selected with no model running is waiting for a
    /// Load, not failing.
    #[test]
    fn synthesis_is_on_when_its_model_runs() {
        assert!(!switched_on(Stage::Synthesis, true, false));
        assert!(switched_on(Stage::Synthesis, true, true));
        assert!(!switched_on(Stage::Synthesis, false, false));
    }

    /// Post-processing keeps the daemon's switch: on with nothing loaded is a
    /// stage meant to run that does not, which its card warns about.
    #[test]
    fn post_processing_keeps_the_daemons_switch() {
        assert!(switched_on(Stage::PostProcessing, true, false));
        assert!(!switched_on(Stage::PostProcessing, false, true));
    }
}
