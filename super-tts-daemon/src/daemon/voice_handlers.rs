// SPDX-License-Identifier: GPL-3.0-only
//! The per-model voice preference: which voice a model speaks in when an
//! utterance names none.
//!
//! The sibling of [`super::language_handlers`], and deliberately shaped the
//! same way — the same four verbs keyed by the same `(source, model)` pair,
//! differing only where the subject matter forces it.
//!
//! A voice was a per-utterance field and nothing else until this existed, which
//! meant only a client that asked for one by name could choose: the CLI's
//! `--voice`, and the Voices page auditioning a clone. Everything else — the
//! settings app's Speak button, a keyboard shortcut, the applet — sent no voice
//! at all and got the manifest's `default_voice`, or, for a model whose voices
//! are all cloned, a refusal telling the user to name one with no way to do it.
//! The preference is stored per model because a voice id means nothing to
//! another model: `ryan` is one of the Qwen `CustomVoice` speakers and no Kokoro
//! build has it, and `voice:<uuid>` is refused outright by a model that does
//! not clone.

use super_tts_registry_types::manifest::VoiceKind;
use super_tts_shared::models::protocol::{Command, DaemonResponse, ErrorCode};

impl crate::daemon::types::SuperTTSDaemon {
    /// Route the four per-model voice commands, keeping their destructuring out
    /// of the giant `handle_command` match.
    ///
    /// # Panics
    /// Panics if `cmd` is not one of the four; the caller only passes those.
    pub async fn handle_model_voice(&self, cmd: Command) -> DaemonResponse {
        match cmd {
            Command::SetModelVoice {
                source,
                model,
                voice,
            } => self.handle_set_model_voice(source, model, voice).await,
            Command::GetModelVoice { source, model } => {
                self.handle_get_model_voice(source, model).await
            }
            Command::ClearModelVoice { source, model } => {
                self.handle_clear_model_voice(source, model).await
            }
            Command::ListModelVoices { source, model } => {
                self.handle_list_model_voices(source, model).await
            }
            _ => unreachable!("handle_model_voice received a non-voice command"),
        }
    }

    /// Build the resolution block for `(source, model)`.
    ///
    /// Says which voice is in effect and where it came from, and deliberately
    /// not which ones could be chosen instead — that is
    /// `GET /pipeline/{stage}/model/{model}/voice/list`, for the same reason
    /// the language pair is split: a picker needs the choices once, where this
    /// block is re-read on every change.
    async fn model_voice_block(&self, source: &str, model: &str) -> Result<serde_json::Value, ()> {
        let def = self.find_model_definition(source, model).await.ok_or(())?;
        // Owned rather than borrowed out of the guard: the guard is a
        // temporary, and the block below outlives this statement.
        let stored = self
            .config
            .read()
            .await
            .model_voice(source, model)
            .map(str::to_owned);
        let default = def.product.default_voice.clone();
        // `effective` is what an utterance naming no voice will actually be
        // spoken in, which is the stored voice if there is one and the
        // manifest's default otherwise. Both can be absent: a model whose
        // voices are all cloned has no default to fall back to, and that is
        // precisely the state where speaking fails until a voice is set — so
        // it is reported rather than papered over.
        let effective = stored.clone().or_else(|| default.clone());
        Ok(serde_json::json!({
            "effective": effective,
            "source": if stored.is_some() { "override" } else { "default" },
            "override": stored,
            "default": default,
            "kinds": def.product.voice_kinds.iter().map(ToString::to_string).collect::<Vec<_>>(),
        }))
    }

    /// The voices `(source, model)` can be pinned to.
    ///
    /// Built from the same rule [`Self::handle_set_model_voice`] enforces, not
    /// from the manifest alone: a model that clones can be pinned to any stored
    /// `voice:<uuid>`, and one whose voices are all cloned or described
    /// declares no presets at all. A picker filled straight from
    /// `[[models.voices]]` would be empty for exactly the models that most need
    /// one, since those are the models that refuse to speak without a voice.
    ///
    /// Described voices are not listed. They are free text — there is no set to
    /// enumerate — so a client offering them offers a text field, which is what
    /// `kinds` in the block above is for.
    pub async fn handle_list_model_voices(&self, source: String, model: String) -> DaemonResponse {
        let Some(def) = self.find_model_definition(&source, &model).await else {
            return DaemonResponse::error_with_code(ErrorCode::InvalidModel, "unknown_model");
        };
        let mut voices: Vec<serde_json::Value> = Vec::new();
        if def.product.voice_kinds.contains(&VoiceKind::Preset) {
            voices.extend(def.product.voices.iter().map(
                |v| serde_json::json!({ "id": v.id, "label": v.display_name(), "kind": "preset" }),
            ));
        }
        if def.product.voice_kinds.contains(&VoiceKind::Cloned) {
            match self.voices.list() {
                Ok(stored) => voices.extend(stored.into_iter().map(|v| {
                    serde_json::json!({
                        "id": format!("{}{}", crate::voices::VOICE_ID_PREFIX, v.id),
                        "label": v.label,
                        "kind": "cloned",
                    })
                })),
                // A library that cannot be read costs the user the cloned half
                // of the list, not the whole endpoint: the presets are still
                // choosable and still correct.
                Err(e) => log::warn!("could not list cloned voices: {e}"),
            }
        }
        DaemonResponse::success().with_available_voices(voices)
    }

    /// Store the voice `(source, model)` speaks in by default.
    ///
    /// Refuses a voice the model cannot resolve rather than storing it: the
    /// alternative is a setting that looks accepted and then fails, or is
    /// silently dropped, on every utterance that follows.
    pub async fn handle_set_model_voice(
        &self,
        source: String,
        model: String,
        voice: String,
    ) -> DaemonResponse {
        let Some(def) = self.find_model_definition(&source, &model).await else {
            return DaemonResponse::error_with_code(ErrorCode::InvalidModel, "unknown_model");
        };
        if let Err(e) = crate::daemon::speech::check_voice(&def, &voice) {
            return DaemonResponse::error_with_code(ErrorCode::InvalidValue, &e.to_string());
        }
        {
            let mut config = self.config.write().await;
            config.update_model_voice(source.clone(), model.clone(), Some(voice.clone()));
        }
        if let Err(e) = self.persist_config().await {
            log::warn!("Failed to persist config after model voice update: {e}");
        }
        log::info!("Set voice {voice} for {model} ({source})");
        self.model_voice_response(&source, &model).await
    }

    /// Read the resolution block. No write, so nothing is persisted.
    pub async fn handle_get_model_voice(&self, source: String, model: String) -> DaemonResponse {
        self.model_voice_response(&source, &model).await
    }

    /// Drop the stored voice, returning the model to its `default_voice`.
    pub async fn handle_clear_model_voice(&self, source: String, model: String) -> DaemonResponse {
        {
            let mut config = self.config.write().await;
            config.update_model_voice(source.clone(), model.clone(), None);
        }
        if let Err(e) = self.persist_config().await {
            log::warn!("Failed to persist config after clearing model voice: {e}");
        }
        log::info!("Cleared the voice for {model} ({source})");
        self.model_voice_response(&source, &model).await
    }

    /// The block as a response, with the one error it can produce.
    async fn model_voice_response(&self, source: &str, model: &str) -> DaemonResponse {
        match self.model_voice_block(source, model).await {
            Ok(block) => DaemonResponse::success().with_voice(block),
            Err(()) => DaemonResponse::error_with_code(ErrorCode::InvalidModel, "unknown_model"),
        }
    }
}
