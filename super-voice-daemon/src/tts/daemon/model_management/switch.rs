// SPDX-License-Identifier: GPL-3.0-only
use crate::tts::daemon::types::SuperTTSDaemon;
use crate::tts::tts_models::ModelDefinition;
use crate::tts::tts_models::backends;
use crate::tts::tts_models::synthesize::Synthesize;
use log::{error, info, warn};
use super_voice_shared::tts::models::protocol::{DaemonResponse, DaemonStatusEvent, ErrorCode};

impl SuperTTSDaemon {
    /// Handle get current model command, and stage 1's model slot with it.
    ///
    /// `current_model` / `current_source` describe the *loaded* model and are
    /// answered exactly as they always were — clients read them, and the idle
    /// case still answers with the error envelope those clients have always
    /// parsed (`GET /active_model` treats it as the normal idle state rather
    /// than a failure). `stage_model` is added beside them and describes the
    /// *selection*: which model the stage points at, whether it is up, on what
    /// device, and what load is in flight.
    ///
    /// The slot is attached to the idle envelope too, deliberately. "Selected
    /// but not loaded" is the one state the split exists to express, and it is
    /// reachable only when nothing is loaded — omitting the slot there would
    /// leave the case that motivates the field as the case no client can read.
    pub async fn handle_get_model(&self) -> DaemonResponse {
        // Built before the read guard below, not inside it: `stage_model` takes
        // the same `model` lock, and tokio's `RwLock` is not reentrant — a
        // writer queued between the two acquisitions would deadlock this
        // handler, and a model switch is exactly what queues one.
        let stage_model = self.stage_model().await;
        let guard = self.model.read().await;

        let response = if let Some(loaded) = guard.as_ref() {
            let name = loaded.definition.name.clone();
            info!("Current model requested: {name}");
            DaemonResponse::success()
                .with_current_model(name.clone())
                .with_current_source(loaded.definition.source.clone())
                .with_message(format!("Current model: {name}"))
        } else {
            warn!("No model is currently loaded");
            DaemonResponse::error("No model is currently loaded")
        };
        DaemonResponse {
            stage_model: Some(stage_model),
            ..response
        }
    }

    /// Stop the utterance in flight, if any, before taking the model down.
    ///
    /// Called by every user action that unloads or replaces the loaded model:
    /// unloading it, deselecting or changing its backend, switching models or
    /// devices, and reloading. These used to be refused while anything was
    /// speaking (`409 speech_in_progress`, "stop it and retry"), which left the
    /// audio playing after an Unload — the button a user reaches for to make
    /// the machine stop — so the refusal read as the button not working. The
    /// latest action wins instead, the same way a new `POST /speak` supersedes
    /// the one before it: the utterance is cancelled, `speaking_state` goes
    /// false, and the mutation goes ahead.
    ///
    /// Cancelling first also frees the `model` lock sooner. Synthesis holds its
    /// read half for a chunk at a time, and a cancelled chunk lets go at the
    /// backend's next frame instead of at the end of the chunk, so the write
    /// the mutation takes next is not left waiting on speech nobody wants.
    ///
    /// `action` is for the log only.
    pub(crate) async fn stop_speech_for(&self, action: &str) {
        if let Some(id) = self.speech.cancel().await {
            info!("stopped utterance {id} to {action}");
        }
    }

    /// `source` (repo id) of the currently selected backend, or `None` when
    /// none is selected — or when the selected install dir is no longer
    /// discovered (the backend was uninstalled out from under the selection).
    ///
    /// `active_backend` stores the relative install dir, not the source, so
    /// this is the mapping every "resolve an omitted `source`" path needs.
    pub(crate) async fn active_backend_source(&self) -> Option<String> {
        let dir_name = self.active_backend.read().await.clone()?;
        let backends = self.backends.read().await;
        backends
            .iter()
            .find(|b| backends::dir_name(b).as_deref() == Some(dir_name.as_str()))
            .map(|b| b.source.clone())
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
        let dir_name = {
            let backends = self.backends.read().await;
            backends
                .iter()
                .find(|b| b.source == source)
                .and_then(backends::dir_name)
        };
        let Some(dir_name) = dir_name else {
            return DaemonResponse::error_with_code(
                ErrorCode::InvalidBackend,
                &format!("No installed backend with source {source} (or its files are missing)"),
            );
        };

        // Always unload when the active backend actually changes — this is the
        // documented postcondition: after `set_active_backend`, the loaded
        // model (if any) is from the requested backend, otherwise the daemon
        // is idle. Same-source re-selects don't disturb a loaded model.
        let prev_dir = self.active_backend.read().await.clone();
        if prev_dir.as_deref() != Some(dir_name.as_str()) {
            self.stop_speech_for("change the backend").await;
            self.unload_current_model().await;
        }

        *self.active_backend.write().await = Some(dir_name.clone());
        self.config
            .write()
            .await
            .update_synthesis_backend(dir_name.clone());
        if let Err(e) = self.persist_config().await {
            warn!("Failed to persist config after active-backend set: {e}");
        }
        self.events
            .publish_synthesis_status(DaemonStatusEvent::ActiveBackendChanged {
                source: Some(source.clone()),
            });
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
        self.stop_speech_for("deselect the backend").await;
        self.unload_current_model().await;
        *self.active_backend.write().await = None;
        self.config.write().await.clear_synthesis_backend();
        if let Err(e) = self.persist_config().await {
            warn!("Failed to persist config after active-backend clear: {e}");
        }
        self.events
            .publish_synthesis_status(DaemonStatusEvent::ActiveBackendChanged { source: None });
        info!("Active backend cleared (daemon idle)");
        DaemonResponse::success().with_message("Active backend cleared".to_string())
    }

    /// Handle set model command — switch to a different model identified by
    /// `(name, source)`.
    pub async fn handle_set_model(&self, model: String, source: String) -> DaemonResponse {
        self.handle_set_model_impl(model, source).await
    }

    async fn handle_set_model_impl(&self, model: String, source: String) -> DaemonResponse {
        info!("Model switch requested: {model} (source={source:?})");
        if let Some(resp) = self.preflight_model_switch(&model, &source).await {
            return resp;
        }

        // `source` is optional on the wire. A switch is a switch *within* a
        // backend, so an omitted one resolves to the active backend rather than
        // to whichever installed backend happens to serve `model` first — two
        // backends may serve the same name, scan order is `read_dir` order, and
        // the backend picked here is persisted below. With nothing selected
        // there is no defensible guess, so the call fails.
        let source = if source.is_empty() {
            let Some(resolved) = self.active_backend_source().await else {
                return DaemonResponse::error_with_code(
                    ErrorCode::InvalidBackend,
                    "No active backend to switch models within. \
                     Select a backend first, or name the model's `source`.",
                );
            };
            info!("Empty source resolved to the active backend: {resolved}");
            resolved
        } else {
            source
        };

        // Resolve against discovered backends; capture the install dir so the
        // backend is recorded below.
        let resolved = {
            let backends = self.backends.read().await;
            backends::find_model(&backends, &model, &source)
                .map(|(b, _)| (b.source.clone(), backends::dir_name(b)))
        };
        let Some((backend_source, backend_dir)) = resolved else {
            return DaemonResponse::error_with_code(
                ErrorCode::InvalidModel,
                &format!(
                    "No installed backend serves {model}. \
                     Install the backend or check the model name."
                ),
            );
        };

        // Selecting a model makes its backend the active one — record this
        // before the load so a load failure leaves the backend selected with no
        // model loaded (rather than silently restoring a previous model).
        if let Some(dir_name) = backend_dir {
            *self.active_backend.write().await = Some(dir_name.clone());
            self.config.write().await.update_synthesis_backend(dir_name);
            // Persist now (not just at finalize) so a subsequent load *failure*
            // still durably records the selected backend across a restart.
            if let Err(e) = self.persist_config().await {
                warn!("Failed to persist config after active-backend select: {e}");
            }
        }

        self.broadcast_model_loading_status(&model);
        self.unload_current_model().await;
        // The model's own device if it has one, else the global default —
        // never the raw default, or a model the user pinned to the CPU would
        // be dragged onto the GPU by the next switch to it.
        let device_pref = self
            .config
            .read()
            .await
            .effective_device(&backend_source, &model);

        match self
            .instantiate_backend(&model, &backend_source, &device_pref)
            .await
        {
            Ok((instance, definition)) => {
                self.finalize_model_switch_success(model, backend_source, definition, instance)
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
        source: String,
        definition: ModelDefinition,
        instance: Box<dyn Synthesize>,
    ) -> DaemonResponse {
        let actual_device = self.finalize_loaded_model(definition, instance).await;
        {
            let mut config_guard = self.config.write().await;
            config_guard.update_synthesis_model(model.clone(), source.clone());
        }
        if let Err(e) = self.persist_config().await {
            warn!("Failed to persist config after model switch: {e}");
        }
        self.broadcast_model_active(&model, &source, &actual_device);
        info!("Switched to model: {model}");
        DaemonResponse::success()
            .with_current_model(model.clone())
            .with_current_source(source)
            .with_message(format!("Successfully switched to model: {model}"))
    }

    async fn preflight_model_switch(&self, model: &str, source: &str) -> Option<DaemonResponse> {
        if let Some(loaded) = self.model.read().await.as_ref()
            && loaded.definition.name == model
            && (source.is_empty() || loaded.definition.source == source)
        {
            info!("Model switch skipped - already using {model}");
            return Some(
                DaemonResponse::success()
                    .with_message(format!("Already using model: {model}"))
                    .with_current_model(loaded.definition.name.clone())
                    .with_current_source(loaded.definition.source.clone()),
            );
        }

        // After the no-op check: asking for the model already loaded does not
        // interrupt it.
        self.stop_speech_for("switch models").await;
        None
    }
}
