// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::types::{SuperSTTDaemon, normalize_device};
use crate::stt_models::transcribe::Transcribe;
use anyhow::Result;
use log::{error, info, warn};
use super_stt_shared::models::protocol::{DaemonResponse, DaemonStatusEvent};
use super_stt_shared::models::provider::Provider;

impl SuperSTTDaemon {
    /// Load a model on an explicit device (used during device switching).
    ///
    /// # Errors
    /// Returns an error if no backend serves the model or instantiation fails.
    pub async fn load_model_with_target_device(
        &self,
        name: &str,
        provider: &Provider,
        source: &str,
        target_device: &str,
    ) -> Result<Box<dyn Transcribe>> {
        info!("Loading model {name} with target device: {target_device}");
        self.broadcast_device_model_loading_status(name, target_device);
        let (instance, _def) = self
            .instantiate_backend(name, provider, source, target_device)
            .await?;
        let actual = normalize_device(&instance.device());
        *self.actual_device.write().await = actual.clone();
        info!("Model {name} loaded on {actual}");
        Ok(instance)
    }

    /// Re-instantiate the currently-loaded model in place (same identity) so a
    /// changed secret or option takes effect. No-op when idle. Rejected during
    /// an active recording. A real-time (WebSocket) session holds the `model`
    /// read lock, so the reload's write-lock acquisition serializes behind it.
    pub async fn handle_reload_active_model(&self) -> DaemonResponse {
        if let Some(resp) = self.guard_model_mutation("reload the model").await {
            return resp;
        }
        let current = self.model.read().await.as_ref().map(|l| {
            (
                l.definition.name.clone(),
                l.definition.provider.clone(),
                l.definition.source.clone(),
            )
        });
        let Some((name, provider, source)) = current else {
            return DaemonResponse::success().with_message("No active model to reload".to_string());
        };

        info!("Reloading active model {name} via {provider} to apply configuration changes");
        self.broadcast_model_loading_status(&name);
        self.unload_current_model().await;
        let device_pref = self.preferred_device.read().await.clone();
        match self
            .instantiate_backend(&name, &provider, &source, &device_pref)
            .await
        {
            Ok((instance, definition)) => {
                self.finalize_model_switch_success(name, provider, source, definition, instance)
                    .await
            }
            Err(e) => {
                error!("Model reload failed: {e}");
                DaemonResponse::error(&format!("Model reload failed: {e}"))
            }
        }
    }

    pub fn broadcast_model_loading_status(&self, model: &str) {
        self.events
            .publish_daemon_status(DaemonStatusEvent::LoadingModel {
                new_model: model.to_string(),
            });
    }

    /// Broadcast model loading status specifically for device switching.
    pub fn broadcast_device_model_loading_status(&self, model: &str, target_device: &str) {
        self.events
            .publish_daemon_status(DaemonStatusEvent::LoadingModelForDevice {
                model: model.to_string(),
                target_device: target_device.to_string(),
            });
    }

    /// Announce that `model` is now the active, loaded model. Emits the
    /// `model_switched` event — which carries the full identity clients use to
    /// update their "current model" view — followed by the operational `ready`
    /// event. Used by both the startup load of the persisted model and
    /// user-initiated switches so the two paths broadcast identical state.
    ///
    /// `source` is included on the wire because a client reconnecting after a
    /// daemon restart has no prior `current_source` to fall back to; without it
    /// the model loads but the settings app keeps showing "no model loaded".
    pub fn broadcast_model_active(
        &self,
        name: &str,
        provider: &Provider,
        source: &str,
        actual_device: &str,
    ) {
        self.events
            .publish_daemon_status(DaemonStatusEvent::ModelSwitched {
                model_name: name.to_string(),
                provider: provider.to_string(),
                source: source.to_string(),
                actual_device: actual_device.to_string(),
            });
        self.events.publish_daemon_status(DaemonStatusEvent::Ready {
            model_loaded: true,
            model_name: Some(name.to_string()),
            actual_device: Some(actual_device.to_string()),
            preferred_device: None,
        });
    }

    // `pub(in crate::daemon)` so the device-switch paths (a sibling module)
    // route their unload through this graceful path instead of dropping the
    // model under the write lock (Tier 3 #2).
    pub(in crate::daemon) async fn unload_current_model(&self) {
        // Take the loaded model OUT of the lock first, then release the lock
        // before calling `shutdown()` — `shutdown()` may take several seconds
        // (e.g. SIGTERM to a CUDA-loaded subprocess that has to free GPU
        // memory) and holding the write lock for that long would stall every
        // other reader. Drop runs after `shutdown()` returns; with the unit
        // already stopped, Drop is effectively a no-op (its second-line stop
        // call is the safety net for crash paths).
        let taken = self.model.write().await.take();
        if let Some(mut loaded) = taken {
            if let Err(e) = loaded.instance.shutdown().await {
                warn!("backend shutdown failed: {e}");
            }
            // Explicit drop here documents the ordering — the loaded value
            // goes away after `shutdown()` ran.
            drop(loaded);
            info!("Current model unloaded");
        }
    }

    /// Install a freshly-loaded model as the active one: record its normalized
    /// device label and write the `LoadedModel` into the slot, returning that
    /// label. The caller must have already emptied the slot gracefully via
    /// [`unload_current_model`], so this assignment drops nothing under the
    /// write lock. Shared by the model-switch and device-switch finalize paths
    /// (Tier 3 #2).
    pub(in crate::daemon) async fn finalize_loaded_model(
        &self,
        definition: crate::stt_models::ModelDefinition,
        instance: Box<dyn Transcribe>,
    ) -> String {
        let actual_device = normalize_device(&instance.device());
        *self.actual_device.write().await = actual_device.clone();
        *self.model.write().await = Some(crate::daemon::types::LoadedModel {
            definition,
            instance,
        });
        actual_device
    }

    /// Unconditional unload used by the daemon's shutdown path. Bypasses the
    /// recording/realtime guard (the process is about to exit anyway) and
    /// gives the loaded model's `Drop` impl a chance to run while the
    /// runtime is still alive — `std::process::exit` later in
    /// `daemon_main` would otherwise skip every destructor, leaving the
    /// `systemd-run --user` subprocess unit orphaned.
    pub async fn shutdown_unload(&self) {
        if self.model.read().await.is_some() {
            info!("Shutdown: unloading current model so subprocess units exit cleanly");
            self.unload_current_model().await;
        }
    }

    /// Drop the currently loaded model. The active backend stays selected
    /// (the user can immediately pick another of its models); to fully idle
    /// out, clear the active backend instead. No-op when no model is loaded.
    /// Rejected during an active recording / real-time session.
    pub async fn handle_unload_active_model(&self) -> DaemonResponse {
        if let Some(resp) = self.switch_guard().await {
            return resp;
        }
        if self.model.read().await.is_none() {
            return DaemonResponse::success().with_message("No model to unload".to_string());
        }
        let dropped = self
            .model
            .read()
            .await
            .as_ref()
            .map(|l| l.definition.name.clone());
        self.unload_current_model().await;
        // Drop the preferred model from config *and persist* so a daemon
        // restart stays idle instead of reloading the just-unloaded model.
        self.config.write().await.clear_preferred_model();
        if let Err(e) = self.persist_config().await {
            warn!("Failed to persist config after unloading model: {e}");
        }
        self.events.publish_daemon_status(DaemonStatusEvent::Ready {
            model_loaded: false,
            model_name: None,
            actual_device: None,
            preferred_device: None,
        });
        let msg = dropped.map_or_else(|| "Model unloaded".to_string(), |n| format!("Unloaded {n}"));
        info!("{msg}");
        DaemonResponse::success().with_message(msg)
    }
}
