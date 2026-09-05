// SPDX-License-Identifier: GPL-3.0-only
//! Handlers for the global + per-model transcription-language endpoints.
//!
//! The per-model handlers are keyed by `(source, model)` and resolve against
//! the **discovered backends** (not the loaded model), so they work for any
//! installed model whether or not it is currently loaded. See
//! `docs/protocol/endpoints/v1/pipeline/language.md`.

use crate::daemon::language::resolve_language;
use crate::daemon::types::SuperSTTDaemon;
use crate::stt_models::ModelDefinition;
use super_stt_shared::models::protocol::{Command, DaemonResponse, ErrorCode};

impl SuperSTTDaemon {
    pub async fn handle_get_primary_language(&self) -> DaemonResponse {
        let config = self.config.read().await;
        let value = config
            .primary_language()
            .map_or(serde_json::Value::Null, |s| {
                serde_json::Value::String(s.to_string())
            });
        DaemonResponse::success().with_language(value)
    }

    /// `GET /settings/language/list` — the tags the global setting accepts.
    #[must_use]
    pub fn handle_list_primary_languages(&self) -> DaemonResponse {
        let languages = std::iter::once("auto".to_string())
            .chain(
                crate::daemon::language::GLOBAL_LANGUAGES
                    .iter()
                    .map(|t| (*t).to_string()),
            )
            .collect();
        DaemonResponse::success()
            .with_available_languages(languages)
            .with_message("Global languages listed".to_string())
    }

    pub async fn handle_set_primary_language(&self, language: String) -> DaemonResponse {
        // The list is the promise, so the setter has to keep it. Before this,
        // `/settings/language` took any string at all — which is why the app
        // shipped its own curated list and no other client could know what to
        // send.
        if !crate::daemon::language::is_offered_globally(&language) {
            return DaemonResponse::error_with_code(
                ErrorCode::UnsupportedLanguage,
                &format!(
                    "Language `{language}` is not one GET /settings/language/list offers. \
                     A model-specific tag belongs on that model, at \
                     POST /pipeline/{{stage}}/model/{{model}}/language."
                ),
            );
        }
        {
            let mut config = self.config.write().await;
            config.update_primary_language(Some(language.clone()));
        }
        if let Err(e) = self.persist_config().await {
            log::warn!("Failed to persist config after primary_language change: {e}");
        }
        self.publish_settings_changed("language");
        DaemonResponse::success().with_language(serde_json::Value::String(language))
    }

    pub async fn handle_clear_primary_language(&self) -> DaemonResponse {
        {
            let mut config = self.config.write().await;
            config.update_primary_language(None);
        }
        if let Err(e) = self.persist_config().await {
            log::warn!("Failed to persist config after primary_language clear: {e}");
        }
        self.publish_settings_changed("language");
        DaemonResponse::success().with_language(serde_json::Value::Null)
    }

    /// Route the three per-model language commands to their handlers. Keeps the
    /// `(source, model)` destructuring out of the giant `handle_command` match.
    ///
    /// # Panics
    /// Panics if `cmd` is not one of the three per-model language variants; the
    /// caller (`handle_command`) only ever passes those.
    pub async fn handle_model_language(&self, cmd: Command) -> DaemonResponse {
        match cmd {
            Command::SetModelLanguage {
                source,
                model,
                language,
            } => {
                self.handle_set_model_language(source, model, language)
                    .await
            }
            Command::GetModelLanguage { source, model } => {
                self.handle_get_model_language(source, model).await
            }
            Command::ClearModelLanguage { source, model } => {
                self.handle_clear_model_language(source, model).await
            }
            Command::ListModelLanguages { source, model } => {
                self.handle_list_model_languages(source, model).await
            }
            _ => unreachable!("handle_model_language received a non-language command"),
        }
    }

    /// Look up a model's [`ModelDefinition`] among the discovered backends by
    /// `(source, model)`. Resolution does **not** require the model to be
    /// loaded — the per-model language endpoint works for any installed model.
    /// The HTTP layer guards `unknown_backend` / `unknown_model` before
    /// dispatch (mirroring options.rs), so a miss here means the backend list
    /// changed between the guard and the handler.
    async fn find_model_definition(&self, source: &str, model: &str) -> Option<ModelDefinition> {
        self.backends
            .read()
            .await
            .iter()
            .find(|b| b.source == source)
            .and_then(|b| b.models.iter().find(|m| m.name == model).cloned())
    }

    /// Build the resolution block for `(source, model)`. Returns `Err` when the
    /// model is not served by any discovered backend (mapped to 404
    /// `unknown_model` by the HTTP layer).
    async fn model_language_block(
        &self,
        source: &str,
        model: &str,
    ) -> Result<serde_json::Value, ()> {
        let def = self.find_model_definition(source, model).await.ok_or(())?;
        let config = self.config.read().await;
        let over = config.model_language(&def.source, &def.name);
        let resolved = resolve_language(
            def.is_multilingual,
            over,
            config.primary_language(),
            &def.supported_languages,
        );
        Ok(serde_json::json!({
            "multilingual": def.is_multilingual,
            "source": resolved.source.as_str(),
            "effective": resolved.wire,
            "override": over,
            "primary": def.primary_language,
        }))
    }

    /// The languages `(source, model)` can be pinned to — what
    /// `GET /pipeline/{stage}/model/{model}/language/list` answers.
    ///
    /// The set [`Self::handle_set_model_language`] accepts, which is not the
    /// manifest's `supported_languages`: `auto` is choosable and is not
    /// declared, and a monolingual model accepts nothing at all however many
    /// tags it lists. A picker filled from the manifest would therefore offer a
    /// value the setter refuses and omit one it takes — so the two are derived
    /// from the same rule, here.
    pub async fn handle_list_model_languages(
        &self,
        source: String,
        model: String,
    ) -> DaemonResponse {
        let Some(def) = self.find_model_definition(&source, &model).await else {
            return DaemonResponse::error_with_code(ErrorCode::InvalidModel, "unknown_model");
        };
        // Empty rather than an error: a monolingual model is a real model with
        // nothing to choose, the way an online model is a real model with no
        // device. Both let a client hide the control on an empty list instead
        // of special-casing a status.
        let languages = if def.is_multilingual {
            std::iter::once("auto".to_string())
                .chain(
                    def.supported_languages
                        .iter()
                        .filter(|tag| !tag.eq_ignore_ascii_case("auto"))
                        .cloned(),
                )
                .collect()
        } else {
            Vec::new()
        };
        DaemonResponse::success()
            .with_available_languages(languages)
            .with_message(format!("Languages available to {model} listed"))
    }

    pub async fn handle_get_model_language(&self, source: String, model: String) -> DaemonResponse {
        match self.model_language_block(&source, &model).await {
            Ok(block) => DaemonResponse::success().with_language(block),
            Err(()) => DaemonResponse::error_with_code(ErrorCode::InvalidModel, "unknown_model"),
        }
    }

    pub async fn handle_set_model_language(
        &self,
        source: String,
        model: String,
        language: String,
    ) -> DaemonResponse {
        // Validate against the named model: it must be multilingual and the tag
        // must be `auto` or one of its supported_languages.
        let Some(def) = self.find_model_definition(&source, &model).await else {
            return DaemonResponse::error_with_code(ErrorCode::InvalidModel, "unknown_model");
        };
        let ok = def.is_multilingual
            && (language == "auto" || def.supported_languages.contains(&language));
        if !ok {
            return DaemonResponse::error_with_code(
                ErrorCode::UnsupportedLanguage,
                "unsupported_language",
            );
        }
        {
            let mut config = self.config.write().await;
            config.update_model_language(source.clone(), model.clone(), Some(language));
        }
        if let Err(e) = self.persist_config().await {
            log::warn!("Failed to persist config after model language change: {e}");
        }
        self.publish_settings_changed("language");
        self.handle_get_model_language(source, model).await
    }

    pub async fn handle_clear_model_language(
        &self,
        source: String,
        model: String,
    ) -> DaemonResponse {
        if self.find_model_definition(&source, &model).await.is_none() {
            return DaemonResponse::error_with_code(ErrorCode::InvalidModel, "unknown_model");
        }
        {
            let mut config = self.config.write().await;
            config.update_model_language(source.clone(), model.clone(), None);
        }
        if let Err(e) = self.persist_config().await {
            log::warn!("Failed to persist config after model language clear: {e}");
        }
        self.publish_settings_changed("language");
        self.handle_get_model_language(source, model).await
    }
}
