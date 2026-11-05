// SPDX-License-Identifier: GPL-3.0-only

#[derive(Debug)]
pub enum Command {
    /// Synthesize `text` with the active model and play it. Cancels whatever
    /// is currently speaking — see `daemon::speech` for the one-at-a-time
    /// policy and why the utterance id exists from the start.
    Speak {
        /// The whole request. How it is spoken — voice, language, rate — is
        /// the user's configuration, read per utterance by the daemon. A
        /// client that can make the machine talk does not thereby get a say
        /// in which voice it talks in; see `docs/protocol/scopes/speak.md`.
        text: String,
    },
    /// Stop the current utterance and discard queued audio.
    StopSpeaking,
    Status,
    SetModel {
        model: String,
        /// Backend repo id that serves the model (e.g.
        /// `github.com/super-tts/openai`). Empty selects the first backend
        /// serving `model`.
        source: String,
    },
    /// The loaded model, and stage 1's model slot beside it: what is
    /// *selected* there, whether that selection is up, the device it runs on,
    /// and the load still in flight.
    ///
    /// The slot reports the selection — the persisted `(model, source)` the
    /// daemon reloads at startup — rather than the loaded instance, and
    /// `loaded` is the separate bit that says whether the instance is running.
    /// Collapsing the two loses the state a card has to render after an
    /// unload: "Kokoro, not loaded", from which re-loading it (on another
    /// device, say) is one click rather than a re-selection the user has to
    /// make from scratch.
    ///
    /// The legacy `current_model` / `current_source` fields are still answered
    /// beside the slot; clients read them and nothing here replaces them.
    GetModel,
    ListModels,
    /// Set the device one model runs on: `cpu` or `gpu`. `model` is resolved
    /// against the active backend. Reloads the model when it is the loaded
    /// one; otherwise only records the choice for its next load, which is what
    /// makes a device picker usable before the model has ever been loaded.
    SetModelDevice {
        model: String,
        device: String,
    },
    /// Read a model's device: the preference in effect for it, what that
    /// resolved to, and the devices this install can offer it.
    GetModelDevice {
        model: String,
    },
    /// The devices this install can offer one model on this host — the
    /// `available_devices` half of [`Command::GetModelDevice`], on its own, so
    /// a client painting a device picker need not also ask what is selected.
    ListModelDevices {
        model: String,
    },
    /// The devices the active backend can be run on here: the union of
    /// [`Command::ListModelDevices`] over the models it serves. Answers the
    /// "what can this backend do on this machine" question a client has before
    /// it has picked a model to ask about.
    ListActiveBackendDevices,
    GetConfig,
    CancelDownload,
    GetDownloadStatus,
    /// Set the per-model language override for a specific `(source, model)`.
    SetModelLanguage {
        source: String,
        model: String,
        language: String,
    },
    /// Read the resolved language block for a specific `(source, model)`.
    GetModelLanguage {
        source: String,
        model: String,
    },
    /// Clear the per-model language override for a specific `(source, model)`.
    ClearModelLanguage {
        source: String,
        model: String,
    },
    /// The languages a specific `(source, model)` can be pinned to.
    ///
    /// The set [`Command::SetModelLanguage`] accepts, which is deliberately not
    /// the manifest's `supported_languages`: `auto` is choosable and no
    /// manifest declares it, and a monolingual model accepts nothing at all
    /// however many tags it happens to list. A picker filled from the manifest
    /// would therefore offer a value the setter refuses and omit the one it
    /// always takes, so both sides are derived from the same rule in the
    /// daemon.
    ListModelLanguages {
        source: String,
        model: String,
    },
    /// Set the voice a specific `(source, model)` speaks in by default.
    SetModelVoice {
        source: String,
        model: String,
        voice: String,
    },
    /// Read the resolved voice block for a specific `(source, model)`.
    GetModelVoice {
        source: String,
        model: String,
    },
    /// Clear the stored voice for a specific `(source, model)`, which returns
    /// it to the manifest's `default_voice`.
    ClearModelVoice {
        source: String,
        model: String,
    },
    /// The voices a specific `(source, model)` can be pinned to.
    ///
    /// Not simply the manifest's `[[models.voices]]`: a model that clones can
    /// also be pinned to a stored `voice:<uuid>`, and one whose voices are all
    /// described or cloned declares no presets at all. Both sides are derived
    /// from the same rule in the daemon, so a picker cannot offer a value the
    /// setter refuses.
    ListModelVoices {
        source: String,
        model: String,
    },
    /// List installed backends with their models, secrets, and options.
    ListBackends,
    /// Re-instantiate the active model in place to apply changed secrets/options.
    ReloadActiveModel,
    /// Unload the currently loaded model. The active backend stays selected
    /// so the user can pick another model from it; to fully idle out, clear
    /// the active backend instead.
    UnloadActiveModel,
    /// Set or clear (empty value) one backend's option override.
    SetBackendOption {
        source: String,
        name: String,
        value: String,
    },
    /// Select the active backend. Does not load a model.
    SetActiveBackend {
        source: String,
    },
    /// Get the active backend.
    GetActiveBackend,
    /// Clear the active backend → unload any model, daemon idle.
    ClearActiveBackend,
    /// Report the synthesis pipeline: every stage in order, with the backend
    /// filling it and whether the user has that stage switched on.
    ///
    /// There is one stage today — stage 1 turns text into audio — and it is
    /// still addressed by position, so a stage put ahead of it (a text
    /// normalizer, an LLM rewriter) becomes a second row in a list clients
    /// already walk rather than a second command they have to learn.
    ///
    /// A stage reports its *backend*, never its model. The model has a runtime
    /// that downloads, allocates, and can fail, and [`Command::GetModel`]
    /// answers for it; reporting the two together is what lets a client read
    /// "a backend is selected" as "a model is loaded", and paint a ready UI
    /// over a daemon that will refuse the next `speak`.
    GetPipeline,
}
