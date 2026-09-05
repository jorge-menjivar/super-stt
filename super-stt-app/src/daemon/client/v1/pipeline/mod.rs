// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline` — the ordered stages a transcript passes through.
//!
//! Mirrors the daemon's `v1/pipeline/` tree: [`stage`] wraps `/pipeline/{stage}`,
//! [`backend`] the menu that fills it, [`model`] wraps
//! `/pipeline/{stage}/model` and its verbs, [`device`] and [`language`] wrap the
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

use super_stt_shared::daemon::http_client::HttpResult;

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
    /// shows comes from `GET /pipeline/{stage}/model/{model}/device` instead,
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
/// (HTTP `GET /pipeline/{stage}` then `GET /pipeline/{stage}/model`).
///
/// Two requests over a Unix socket, which is what a card needs and what the
/// split costs. A failure in either fails the read, since a half-drawn card is
/// worse than one that reports it could not load.
pub async fn get_stage_view(stage: u32) -> HttpResult<StageState> {
    let backend = stage::get_stage(stage).await?;
    let model = model::get_stage_model(stage).await?;
    Ok(StageState {
        source: backend.source,
        name: backend.name,
        enabled: backend.enabled,
        model: model.model,
        loaded: model.loaded,
        device: model.device,
    })
}
