// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::types::{LoadedModel, SuperSTTDaemon, normalize_device};
use crate::stt_models::backends;
use crate::stt_models::transcribe::Transcribe;
use chrono::Utc;
use log::{error, info, warn};
use super_stt_shared::models::protocol::DaemonResponse;
use super_stt_shared::models::provider::Provider;
use super_stt_shared::models::registry::ModelDefinition;

impl SuperSTTDaemon {
    /// Handle get current model command.
    pub async fn handle_get_model(&self) -> DaemonResponse {
        let guard = self.model.read().await;

        if let Some(loaded) = guard.as_ref() {
            let name = loaded.definition.name.clone();
            info!("Current model requested: {name}");
            DaemonResponse::success()
                .with_current_model(name.clone())
                .with_current_provider(loaded.definition.provider.clone())
                .with_current_source(loaded.definition.source.clone())
                .with_message(format!("Current model: {name}"))
        } else {
            warn!("No model is currently loaded");
            DaemonResponse::error("No model is currently loaded")
        }
    }

    /// Recording/realtime guard shared by backend + model switching.
    pub(super) async fn switch_guard(&self) -> Option<DaemonResponse> {
        if *self.busy.read().await {
            return Some(DaemonResponse::error(
                "Cannot change the backend during active recording. Please wait for it to finish.",
            ));
        }
        if !self.realtime_manager.get_active_sessions().await.is_empty() {
            return Some(DaemonResponse::error(
                "Cannot change the backend during active real-time transcription sessions.",
            ));
        }
        None
    }

    /// Build the `{source, name, model_loaded}` payload for the backend at the
    /// given relative install dir, or `None` if it isn't currently discovered.
    async fn active_backend_payload(&self, dir_name: &str) -> Option<serde_json::Value> {
        let backends = self.backends.read().await;
        let backend = backends
            .iter()
            .find(|b| backends::dir_name(b).as_deref() == Some(dir_name))?;
        let model_loaded = self
            .model
            .read()
            .await
            .as_ref()
            .is_some_and(|m| m.definition.source == backend.source);
        Some(serde_json::json!({
            "source": backend.source,
            "name": backend.name,
            "model_loaded": model_loaded,
        }))
    }

    /// Select the active backend by its `source` (repo id). Validates that the
    /// backend is installed; unloads the currently-loaded model whenever the
    /// active dir actually changes, so `/active_model` is `null` immediately
    /// after a switch. A redundant call with the same source is a no-op for
    /// the loaded model. Does not load a model — only `set_model` can fail at
    /// runtime.
    pub async fn handle_set_active_backend(&self, source: String) -> DaemonResponse {
        if let Some(resp) = self.switch_guard().await {
            return resp;
        }
        let dir_name = {
            let backends = self.backends.read().await;
            backends
                .iter()
                .find(|b| b.source == source)
                .and_then(backends::dir_name)
        };
        let Some(dir_name) = dir_name else {
            return DaemonResponse::error(&format!(
                "No installed backend with source {source} (or its files are missing)"
            ));
        };

        // Always unload when the active backend actually changes — this is the
        // documented postcondition: after `set_active_backend`, the loaded
        // model (if any) is from the requested backend, otherwise the daemon
        // is idle. Same-source re-selects don't disturb a loaded model.
        let prev_dir = self.active_backend.read().await.clone();
        if prev_dir.as_deref() != Some(dir_name.as_str()) {
            self.unload_current_model().await;
        }

        *self.active_backend.write().await = Some(dir_name.clone());
        self.config
            .write()
            .await
            .update_active_backend(dir_name.clone());
        self.events
            .publish_daemon_status_changed(serde_json::json!({
                "status": "active_backend_changed",
                "source": source,
                "timestamp": Utc::now().to_rfc3339(),
            }));
        info!("Active backend set to {source}");

        let payload = self
            .active_backend_payload(&dir_name)
            .await
            .unwrap_or(serde_json::Value::Null);
        DaemonResponse::success()
            .with_active_backend(payload)
            .with_message(format!("Active backend: {source}"))
    }

    /// Report the active backend (`{source, name, model_loaded}`, or null/idle).
    pub async fn handle_get_active_backend(&self) -> DaemonResponse {
        let dir_name = self.active_backend.read().await.clone();
        let payload = match dir_name {
            Some(d) => self.active_backend_payload(&d).await,
            None => None,
        };
        DaemonResponse::success().with_active_backend(payload.unwrap_or(serde_json::Value::Null))
    }

    /// Clear the active backend: unload any model and return to idle.
    pub async fn handle_clear_active_backend(&self) -> DaemonResponse {
        if let Some(resp) = self.switch_guard().await {
            return resp;
        }
        self.unload_current_model().await;
        *self.active_backend.write().await = None;
        self.config.write().await.clear_active_backend();
        self.events
            .publish_daemon_status_changed(serde_json::json!({
                "status": "active_backend_changed",
                "source": serde_json::Value::Null,
                "timestamp": Utc::now().to_rfc3339(),
            }));
        info!("Active backend cleared (daemon idle)");
        DaemonResponse::success().with_message("Active backend cleared".to_string())
    }

    /// Handle set model command — switch to a different model identified by
    /// `(name, provider, source)`.
    pub async fn handle_set_model(
        &self,
        model: String,
        provider: Provider,
        source: String,
    ) -> DaemonResponse {
        self.handle_set_model_impl(model, provider, source).await
    }

    async fn handle_set_model_impl(
        &self,
        model: String,
        provider: Provider,
        source: String,
    ) -> DaemonResponse {
        info!("Model switch requested: {model} via {provider} (source={source:?})");
        if let Some(resp) = self
            .preflight_model_switch(&model, &provider, &source)
            .await
        {
            return resp;
        }

        // Resolve against discovered backends; capture the concrete source +
        // install dir so the backend is recorded even when the request left
        // `source` empty. Online-ness is only knowable from the resolved model
        // (the `provider` string no longer encodes it), so capture it here.
        let resolved = {
            let backends = self.backends.read().await;
            backends::find_model(&backends, &model, &provider, &source)
                .map(|(b, d)| (b.source.clone(), backends::dir_name(b), d.is_online()))
        };
        let Some((backend_source, backend_dir, is_online)) = resolved else {
            return DaemonResponse::error(&format!(
                "No installed backend serves {model} via {provider}. \
                 Install the backend or check the model name."
            ));
        };

        // Online models must be explicitly enabled — gated after resolution
        // since online-ness is a property of the resolved model.
        if is_online && !self.config.read().await.online.allow_online_models {
            return DaemonResponse::error(
                "Online models are disabled. Enable 'Allow Online Models' in settings first.",
            );
        }

        // Selecting a model makes its backend the active one — record this
        // before the load so a load failure leaves the backend selected with no
        // model loaded (rather than silently restoring a previous model).
        if let Some(dir_name) = backend_dir {
            *self.active_backend.write().await = Some(dir_name.clone());
            self.config.write().await.update_active_backend(dir_name);
        }

        self.broadcast_model_loading_status(&model);
        self.unload_current_model().await;
        let device_pref = self.preferred_device.read().await.clone();

        match self
            .instantiate_backend(&model, &provider, &backend_source, &device_pref)
            .await
        {
            Ok((instance, definition)) => {
                self.finalize_model_switch_success(
                    model,
                    provider,
                    backend_source,
                    definition,
                    instance,
                )
                .await
            }
            Err(e) => {
                error!("Model switch failed: {e}");
                DaemonResponse::error(&format!("Model switch failed: {e}"))
            }
        }
    }

    pub(super) async fn finalize_model_switch_success(
        &self,
        model: String,
        provider: Provider,
        source: String,
        definition: ModelDefinition,
        instance: Box<dyn Transcribe>,
    ) -> DaemonResponse {
        let actual_device = normalize_device(&instance.device());
        *self.actual_device.write().await = actual_device.clone();
        *self.model.write().await = Some(LoadedModel {
            definition,
            instance,
        });
        {
            let mut config_guard = self.config.write().await;
            config_guard.update_preferred_model(model.clone(), provider.clone(), source.clone());
        }
        if let Err(e) = self.persist_config().await {
            warn!("Failed to persist config after model switch: {e}");
        }
        self.broadcast_model_active(&model, &provider, &source, &actual_device);
        info!("Switched to model: {model} via {provider}");
        DaemonResponse::success()
            .with_current_model(model.clone())
            .with_current_provider(provider)
            .with_current_source(source)
            .with_message(format!("Successfully switched to model: {model}"))
    }

    async fn preflight_model_switch(
        &self,
        model: &str,
        provider: &Provider,
        source: &str,
    ) -> Option<DaemonResponse> {
        if *self.busy.read().await {
            warn!("Model switch rejected - recording in progress");
            return Some(DaemonResponse::error(
                "Cannot switch models during active recording. Please wait for recording to complete.",
            ));
        }
        let active_sessions = self.realtime_manager.get_active_sessions().await;
        if !active_sessions.is_empty() {
            warn!(
                "Model switch rejected - {} real-time transcription sessions active",
                active_sessions.len()
            );
            return Some(DaemonResponse::error(&format!(
                "Cannot switch models during active real-time transcription sessions. {} active sessions: {}. Please stop all sessions first.",
                active_sessions.len(),
                active_sessions.join(", ")
            )));
        }
        if let Some(loaded) = self.model.read().await.as_ref()
            && loaded.definition.name == model
            && loaded.definition.provider == *provider
            && (source.is_empty() || loaded.definition.source == source)
        {
            info!("Model switch skipped - already using {model} via {provider}");
            return Some(
                DaemonResponse::success()
                    .with_message(format!("Already using model: {model}"))
                    .with_current_model(loaded.definition.name.clone())
                    .with_current_provider(loaded.definition.provider.clone())
                    .with_current_source(loaded.definition.source.clone()),
            );
        }

        None
    }
}
