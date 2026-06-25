// SPDX-License-Identifier: GPL-3.0-only
//! Handlers for the global + per-model transcription-language endpoints.

use crate::daemon::language::resolve_language;
use crate::daemon::types::SuperSTTDaemon;
use super_stt_shared::models::protocol::DaemonResponse;

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

    pub async fn handle_set_primary_language(&self, language: String) -> DaemonResponse {
        {
            let mut config = self.config.write().await;
            config.update_primary_language(Some(language.clone()));
        }
        if let Err(e) = self.persist_config().await {
            log::warn!("Failed to persist config after primary_language change: {e}");
        }
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
        DaemonResponse::success().with_language(serde_json::Value::Null)
    }

    /// Build the resolution block for the active model. Returns `Err` when idle
    /// (mapped to 409 via `CONFLICT_PHRASES` — see dispatch.rs).
    async fn active_model_language_block(&self) -> Result<serde_json::Value, ()> {
        let def = {
            let guard = self.model.read().await;
            match guard.as_ref() {
                Some(loaded) => loaded.definition.clone(),
                None => return Err(()),
            }
        };
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
            "supported": def.supported_languages,
        }))
    }

    pub async fn handle_get_active_model_language(&self) -> DaemonResponse {
        match self.active_model_language_block().await {
            Ok(block) => DaemonResponse::success().with_language(block),
            Err(()) => DaemonResponse::error("not_ready"),
        }
    }

    pub async fn handle_set_active_model_language(&self, language: String) -> DaemonResponse {
        // Validate against the active model (must be multilingual; tag must be
        // `auto` or in supported_languages).
        let def = {
            let guard = self.model.read().await;
            match guard.as_ref() {
                Some(loaded) => loaded.definition.clone(),
                None => return DaemonResponse::error("not_ready"),
            }
        };
        let ok = def.is_multilingual
            && (language == "auto" || def.supported_languages.contains(&language));
        if !ok {
            return DaemonResponse::error("unsupported_language");
        }
        {
            let mut config = self.config.write().await;
            config.update_model_language(def.source.clone(), def.name.clone(), Some(language));
        }
        if let Err(e) = self.persist_config().await {
            log::warn!("Failed to persist config after model language change: {e}");
        }
        self.handle_get_active_model_language().await
    }

    pub async fn handle_clear_active_model_language(&self) -> DaemonResponse {
        let def = {
            let guard = self.model.read().await;
            match guard.as_ref() {
                Some(loaded) => loaded.definition.clone(),
                None => return DaemonResponse::error("not_ready"),
            }
        };
        {
            let mut config = self.config.write().await;
            config.update_model_language(def.source.clone(), def.name.clone(), None);
        }
        if let Err(e) = self.persist_config().await {
            log::warn!("Failed to persist config after model language clear: {e}");
        }
        self.handle_get_active_model_language().await
    }
}
