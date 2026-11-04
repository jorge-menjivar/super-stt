// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline` — the ordered stages an utterance passes through.
//!
//! Mirrors the daemon's `v1/pipeline/` tree: [`stage`] wraps `/pipeline/{stage}`,
//! [`backend`] the menu that fills it, [`model`] wraps `/pipeline/{stage}/model`
//! and its verbs, [`device`], [`language`] and [`voice`] the per-model
//! preferences.
//!
//! Super TTS has exactly one stage — `SYNTHESIS_STAGE`, text in and audio out —
//! and every call here still takes the position as a parameter. That is not
//! ceremony: the daemon addresses stages by number, and a client that baked `1`
//! into each path would have to be rewritten rather than re-called the day a
//! stage is inserted ahead of synthesis. Passing it through costs one argument
//! and keeps the app's shape the same as the wire's.
//!
//! [`StageState`] sits here, above the path modules, because it belongs to no
//! single path: a card draws a stage's backend and its model together, and
//! those are two endpoints. They are two endpoints because they are two
//! different things — a backend selection that outlives everything, and a model
//! that has a runtime and can fail to come up — and joining them in the client
//! is cheaper than the drift that came of joining them on the wire.

pub(crate) mod backend;
pub(crate) mod device;
pub(crate) mod language;
pub(crate) mod model;
pub(crate) mod stage;
pub(crate) mod voice;

use super_tts_shared::daemon::http_client::HttpResult;

pub use model::StageDevice;

/// One pipeline stage as a card renders it.
///
/// The union of [`stage::StageBackend`] and [`model::StageModel`], fetched by
/// [`get_stage_view`]. Keeping the two halves distinct in the types would push
/// the join into every view; keeping them distinct on the wire is what stops a
/// client reading "a backend is chosen" as "a model is running", which are the
/// two states a Models card most needs to tell apart.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StageState {
    /// The backend filling the stage; `None` when the stage is empty.
    pub source: Option<String>,
    /// That backend's display name; `None` when the stage is empty.
    pub name: Option<String>,
    /// Whether the user has this stage switched on.
    ///
    /// Not the same as [`Self::loaded`]: a stage can be switched on while its
    /// model failed to come up, and the card says so rather than silently
    /// looking idle.
    pub enabled: bool,
    /// The model the stage is pointed at; `None` when none is picked. Survives
    /// an unload, so the card can offer to load the same model again without
    /// the user picking it a second time.
    pub model: Option<String>,
    /// Whether that model is up. This is what `POST /speak` requires.
    pub loaded: bool,
    /// The device the selection runs on; `None` when nothing is selected.
    ///
    /// Read through [`Self::running_device`]. The value a device *picker* shows
    /// comes from [`device::get_model_device`] instead, alongside the list it
    /// offers — one answer for one control, rather than the preference arriving
    /// by one route and the options by another.
    pub device: Option<StageDevice>,
}

impl StageState {
    /// The selected `(model, source)` pair, when the selection is complete.
    ///
    /// Both halves are needed to name a model on the wire, and they arrive from
    /// different endpoints: the model slot no longer carries a `source`, since
    /// the backend is the stage's property and not the model's. A stage with a
    /// backend but no model picked has no pair to give.
    #[must_use]
    pub fn selection(&self) -> Option<(String, String)> {
        Some((self.model.clone()?, self.source.clone()?))
    }

    /// The accelerator the model is actually running on, or `None` when it is
    /// not running.
    ///
    /// The stored preference is not it: a `gpu` choice that fell back to the
    /// CPU still reads `gpu`, and a card that suffixed its "Active:" line with
    /// the preference would tell the user they are on a GPU they are not on.
    /// `none` is filtered out too — that is what an online model reports, and
    /// it is not a device anyone wants shown.
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
/// split costs. A failure in either fails the read, since a half-drawn card —
/// a backend name above an empty model row that is empty only because the
/// second request failed — is worse than one that reports it could not load.
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
