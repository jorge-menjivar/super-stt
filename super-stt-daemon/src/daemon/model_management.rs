// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::SuperSTTDaemon;
use crate::download_progress::DownloadProgressTracker;
use crate::stt_models::local::{voxtral::VoxtralModel, whisper::WhisperModel};
use crate::stt_models::transcribe::Transcribe;
use anyhow::Result;
use chrono::Utc;
use log::{error, info, warn};
use std::sync::Arc;
use super_stt_shared::models::protocol::DaemonResponse;
use super_stt_shared::models::provider::{OnlineProvider, Provider};
use super_stt_shared::models::registry::{self, SourceKind};

impl SuperSTTDaemon {
    /// Load model with explicit target device (used during device switching)
    ///
    /// # Errors
    ///
    /// Returns an error if model loading fails on both the requested device
    /// and any attempted fallback.
    pub async fn load_model_with_target_device(
        &self,
        name: &str,
        provider: Provider,
        source: SourceKind,
        target_device: &str,
    ) -> Result<Box<dyn Transcribe>> {
        let name_owned = name.to_string();
        let target_device_copy = target_device.to_string();
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        info!("Loading model with target device: {target_device}");

        // Broadcast model loading status for device switch
        self.broadcast_device_model_loading_status(name_owned.clone(), target_device)
            .await;

        // Customs carry their disk path; built-ins load via the HF cache.
        let custom_path = if matches!(source, SourceKind::Custom) {
            self.custom_models
                .read()
                .await
                .iter()
                .find(|m| m.name == name && m.provider == provider)
                .map(|m| m.path.clone())
        } else {
            None
        };

        // Load model in a single blocking task with cancellation support
        let mut load_handle = tokio::task::spawn_blocking(move || {
            Self::load_model_sync(name_owned, provider, custom_path, &target_device_copy)
        });

        // Wait for either model loading completion, shutdown signal, or timeout (60 seconds)
        let model_result = tokio::select! {
            result = &mut load_handle => {
                result.map_err(|e| anyhow::anyhow!("Model loading task failed: {e}"))?
            }
            _ = shutdown_rx.recv() => {
                warn!("Model loading cancelled due to shutdown - aborting blocking task");
                load_handle.abort();
                return Err(anyhow::anyhow!("Model loading cancelled due to shutdown"));
            }
            () = tokio::time::sleep(tokio::time::Duration::from_mins(1)) => {
                error!("Model loading timed out after 60 seconds - aborting blocking task");
                load_handle.abort();
                return Err(anyhow::anyhow!("Model loading timed out"));
            }
        }?;

        // Update actual device based on what was loaded
        let actual_device_str = match model_result.device() {
            candle_core::Device::Cpu => "cpu",
            candle_core::Device::Cuda(_) => "cuda",
            candle_core::Device::Metal(_) => "metal",
        };

        *self.actual_device.write().await = actual_device_str.to_string();

        if actual_device_str != target_device && target_device == "cuda" {
            warn!("CUDA loading failed, successfully fell back to CPU");
            info!("Model loaded on CPU fallback device");
        } else {
            info!("Model loaded successfully on {actual_device_str} device");
        }

        Ok(model_result)
    }

    /// Load model with device preference and fallback handling
    ///
    /// # Errors
    ///
    /// Returns an error if model loading fails on both the preferred device
    /// and the CPU fallback (if attempted).
    pub async fn load_model_with_device_preference(
        &self,
        name: &str,
        provider: Provider,
        source: SourceKind,
    ) -> Result<Box<dyn Transcribe>> {
        let preferred_device = self.preferred_device.read().await.clone();
        self.load_model_with_target_device(name, provider, source, &preferred_device)
            .await
    }

    /// Handle get current model command
    pub async fn handle_get_model(&self) -> DaemonResponse {
        let guard = self.model.read().await;

        if let Some(loaded) = guard.as_ref() {
            let name = loaded.definition.name.to_string();
            info!("Current model requested: {name}");
            DaemonResponse::success()
                .with_current_model(name.clone())
                .with_current_provider(loaded.definition.provider)
                .with_current_source(loaded.definition.source.kind())
                .with_message(format!("Current model: {name}"))
        } else {
            warn!("No model is currently loaded");
            DaemonResponse::error("No model is currently loaded")
        }
    }

    /// Handle set model command - switch to a different model identified by
    /// `(name, provider, source)`.
    pub async fn handle_set_model(
        &self,
        model: String,
        provider: Provider,
        source: SourceKind,
    ) -> DaemonResponse {
        self.handle_set_model_impl(model, provider, source).await
    }

    /// Internal implementation for model switching (split to reduce public fn size)
    async fn handle_set_model_impl(
        &self,
        model: String,
        provider: Provider,
        source: SourceKind,
    ) -> DaemonResponse {
        info!("Model switch requested: {model} via {provider} ({source})");
        if let Some(resp) = self
            .preflight_model_switch(model.clone(), provider, source)
            .await
        {
            return resp;
        }

        // Resolve via (name, provider, source): registry or custom_models hit → ok;
        // miss → error.
        let Some(_definition) = self.resolve_definition(&model, provider, source).await else {
            return DaemonResponse::error(&format!(
                "Unknown model: {model} via {provider} ({source}). Not in registry or custom_models_dir."
            ));
        };

        // Online providers have a separate fast path (no download needed)
        if let Provider::Online(online) = provider {
            return self.handle_set_online_model(model, online).await;
        }

        // Custom models: load directly from disk (no download)
        if matches!(source, SourceKind::Custom) {
            let name = model.clone();
            return self.handle_set_custom_model(&name, model, provider).await;
        }

        // Standard local models: download then load
        let previous_model = {
            let guard = self.model.read().await;
            guard.as_ref().map(|loaded| {
                (
                    loaded.definition.name.to_string(),
                    loaded.definition.provider,
                )
            })
        };

        self.broadcast_model_loading_status(model.clone()).await;
        let tracker = self.create_progress_tracker(&model);
        if let Err(resp) = self.register_download(&tracker) {
            tracker.cancel();
            return *resp;
        }
        self.unload_current_model().await;
        let start_time = std::time::Instant::now();
        match self
            .download_and_load_model(model.clone(), provider, Arc::clone(&tracker), start_time)
            .await
        {
            Ok(instance) => {
                self.finalize_model_switch_success(model, provider, instance, &tracker)
                    .await
            }
            Err(e) => {
                error!("Model switch failed: {e}");
                self.download_manager.clear_download();
                if let Some((prev, prev_provider)) = previous_model {
                    warn!("Restoring previous model: {prev} via {prev_provider}");
                    self.restore_model(prev, prev_provider).await;
                }
                DaemonResponse::error(&format!("Model switch failed: {e}"))
            }
        }
    }

    /// Handle switching to a custom local model (no download, load from disk).
    async fn handle_set_custom_model(
        &self,
        name: &str,
        model: String,
        provider: Provider,
    ) -> DaemonResponse {
        // Look up the custom model by (name, provider).
        let custom_models = self.custom_models.read().await;
        let info = match custom_models
            .iter()
            .find(|m| m.name == name && m.provider == provider)
        {
            Some(info) => info.clone(),
            None => {
                return DaemonResponse::error(&format!(
                    "Custom model '{name}' via {provider} not found. Check custom_models_dir."
                ));
            }
        };
        drop(custom_models);

        let previous_model = {
            let guard = self.model.read().await;
            guard.as_ref().map(|loaded| {
                (
                    loaded.definition.name.to_string(),
                    loaded.definition.provider,
                )
            })
        };
        self.broadcast_model_loading_status(model.clone()).await;
        self.unload_current_model().await;

        let preferred_device = self.preferred_device.read().await.clone();
        let custom_path = info.path.clone();
        let name_owned = name.to_string();
        let start_time = std::time::Instant::now();

        let result = tokio::task::spawn_blocking(move || {
            let result =
                Self::load_model_sync(name_owned, provider, Some(custom_path), &preferred_device);
            let duration = start_time.elapsed();
            info!("Custom model loading completed in {duration:?}");
            result
        })
        .await;

        match result {
            Ok(Ok(instance)) => {
                // Update actual device
                let actual_device_str = match instance.device() {
                    candle_core::Device::Cpu => "cpu",
                    candle_core::Device::Cuda(_) => "cuda",
                    candle_core::Device::Metal(_) => "metal",
                };
                *self.actual_device.write().await = actual_device_str.to_string();

                let definition = super_stt_shared::models::registry::ModelDefinition::custom(
                    name,
                    info.path.clone(),
                    provider,
                );
                *self.model.write().await = Some(crate::daemon::types::LoadedModel {
                    definition,
                    instance,
                });

                info!("Custom model '{name}' loaded on {actual_device_str}");
                DaemonResponse::success()
                    .with_current_model(model)
                    .with_current_provider(provider)
                    .with_message(format!("Successfully loaded custom model: {name}"))
            }
            Ok(Err(e)) => {
                let err_msg = format!("Failed to load custom model '{name}': {e}");
                error!("{err_msg}");
                if let Some((prev, prev_provider)) = previous_model {
                    warn!("Restoring previous model: {prev} via {prev_provider}");
                    self.restore_model(prev, prev_provider).await;
                }
                DaemonResponse::error(&err_msg)
            }
            Err(e) => {
                let err_msg = format!("Failed to load custom model '{name}': {e}");
                error!("{err_msg}");
                if let Some((prev, prev_provider)) = previous_model {
                    warn!("Restoring previous model: {prev} via {prev_provider}");
                    self.restore_model(prev, prev_provider).await;
                }
                DaemonResponse::error(&err_msg)
            }
        }
    }
}

impl SuperSTTDaemon {
    /// Handle switching to an online model (no download, instant creation).
    async fn handle_set_online_model(
        &self,
        model: String,
        online: OnlineProvider,
    ) -> DaemonResponse {
        // Guard: online models must be explicitly enabled
        let config = self.config.read().await;
        if !config.online.allow_online_models {
            return DaemonResponse::error(
                "Online models are disabled. Enable 'Allow Online Models' in settings first.",
            );
        }
        drop(config);

        // Guard: API key must be configured in the system keyring
        let key_name = online.api_key_name();
        let api_key = match crate::keyring::get_api_key(key_name) {
            Ok(Some(key)) if !key.is_empty() => key,
            Ok(_) => {
                return DaemonResponse::error(&format!(
                    "{key_name} API key is not configured. Add your API key in the Online Models settings."
                ));
            }
            Err(e) => {
                return DaemonResponse::error(&format!("Failed to read API key from keyring: {e}"));
            }
        };

        let provider = Provider::Online(online);
        let Some(definition) = registry::find_by(&model, provider).cloned() else {
            return DaemonResponse::error(&format!(
                "Model {model} is not available via {provider}"
            ));
        };

        self.broadcast_model_loading_status(model.clone()).await;
        self.unload_current_model().await;

        let instance = match Self::create_online_instance(online, api_key, &model) {
            Ok(inst) => inst,
            Err(e) => {
                return DaemonResponse::error(&format!("Failed to create online model: {e}"));
            }
        };

        *self.model.write().await = Some(crate::daemon::types::LoadedModel {
            definition,
            instance,
        });
        {
            let mut config_guard = self.config.write().await;
            config_guard.update_preferred_model(model.clone(), provider, SourceKind::Online);
        }
        if let Err(e) = self.broadcast_config_change().await {
            warn!("Failed to broadcast config change after online model switch: {e}");
        }
        let _ = self
            .notification_manager
            .broadcast_event(
                "daemon_status_changed".to_string(),
                "daemon".to_string(),
                serde_json::json!({
                    "status": "ready",
                    "model_loaded": true,
                    "provider": provider.to_string(),
                    "model_name": model.clone(),
                    "timestamp": chrono::Utc::now().to_rfc3339()
                }),
            )
            .await;

        info!("Switched to online model: {model} via {provider}");
        DaemonResponse::success()
            .with_current_model(model.clone())
            .with_current_provider(provider)
            .with_message(format!("Successfully switched to online model: {model}"))
    }

    async fn preflight_model_switch(
        &self,
        model: String,
        provider: Provider,
        source: SourceKind,
    ) -> Option<DaemonResponse> {
        if *self.is_recording.read().await {
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
            && loaded.definition.provider == provider
            && loaded.definition.source.kind() == source
        {
            info!("Model switch skipped - already using {model} via {provider} ({source})");
            return Some(
                DaemonResponse::success()
                    .with_message(format!("Already using model: {model}"))
                    .with_current_model(loaded.definition.name.to_string())
                    .with_current_provider(loaded.definition.provider),
            );
        }

        None
    }

    pub async fn broadcast_model_loading_status(&self, model: String) {
        if let Err(e) = self
            .notification_manager
            .broadcast_event(
                "daemon_status_changed".to_string(),
                "daemon".to_string(),
                serde_json::json!({
                    "status": "loading_model",
                    "new_model": model.clone(),
                    "timestamp": Utc::now().to_rfc3339()
                }),
            )
            .await
        {
            warn!("Failed to broadcast model loading status: {e}");
        }
    }

    /// Broadcast model loading status specifically for device switching
    pub async fn broadcast_device_model_loading_status(&self, model: String, target_device: &str) {
        if let Err(e) = self
            .notification_manager
            .broadcast_event(
                "daemon_status_changed".to_string(),
                "daemon".to_string(),
                serde_json::json!({
                    "status": "loading_model_for_device",
                    "model": model.clone(),
                    "target_device": target_device,
                    "timestamp": Utc::now().to_rfc3339()
                }),
            )
            .await
        {
            warn!("Failed to broadcast device model loading status: {e}");
        }
    }

    #[must_use]
    pub fn create_progress_tracker(&self, model: &str) -> Arc<DownloadProgressTracker> {
        let flag = self.download_manager.get_cancellation_flag();
        Arc::new(
            DownloadProgressTracker::new(model.to_string(), 0, Arc::clone(&flag))
                .with_notification_manager(Arc::clone(&self.notification_manager)),
        )
    }

    /// Register a download tracker with the download manager.
    ///
    /// # Errors
    /// Restore a previously loaded model after a failed switch.
    /// Best-effort: if this also fails, the daemon remains without a model.
    async fn restore_model(&self, model: String, provider: Provider) {
        let tracker = self.create_progress_tracker(&model);
        if self.register_download(&tracker).is_err() {
            error!("Cannot restore model: another download in progress");
            return;
        }
        let start_time = std::time::Instant::now();
        match self
            .download_and_load_model(model.clone(), provider, Arc::clone(&tracker), start_time)
            .await
        {
            Ok(instance) => {
                tracker.mark_completed();
                self.download_manager.clear_download();
                let Some(definition) = registry::find_by(&model, provider).cloned() else {
                    error!("Cannot restore model {model} via {provider}: not in registry");
                    return;
                };
                *self.model.write().await = Some(crate::daemon::types::LoadedModel {
                    definition,
                    instance,
                });
                info!("Previous model {model} via {provider} restored successfully");
            }
            Err(e) => {
                self.download_manager.clear_download();
                error!("Failed to restore previous model {model}: {e}");
            }
        }
    }

    /// # Errors
    ///
    /// Returns an error if another download is already in progress.
    pub fn register_download(
        &self,
        tracker: &Arc<DownloadProgressTracker>,
    ) -> Result<(), Box<DaemonResponse>> {
        self.download_manager
            .start_download(Arc::clone(tracker))
            .map_err(|e| {
                warn!("Failed to register download: {e}");
                Box::new(DaemonResponse::error(&format!(
                    "Another download is in progress: {e}"
                )))
            })
    }

    async fn unload_current_model(&self) {
        let mut model_guard = self.model.write().await;
        *model_guard = None;
        info!("Current model unloaded");
    }

    /// Synchronous model loader. Handles device preference, VRAM preflight, and
    /// CUDA→CPU fallback. Routes to either Whisper or Voxtral based on the
    /// resolved family (registry definition for built-ins, detected
    /// architecture for custom models). Designed to run inside `spawn_blocking`.
    fn load_model_sync(
        name: String,
        provider: Provider,
        custom_path: Option<std::path::PathBuf>,
        preferred_device: &str,
    ) -> Result<Box<dyn Transcribe>> {
        let force_cpu = preferred_device == "cpu";
        info!(
            "Loading model '{name}' via {provider} (custom={}) on {preferred_device}",
            custom_path.is_some()
        );

        // VRAM preflight only applies to built-ins (we know their estimated size).
        if custom_path.is_none()
            && let Some(def) = registry::find_by(&name, provider)
        {
            Self::preflight_vram_check(def, force_cpu)?;
        }

        let load = move |cpu: bool| -> Result<Box<dyn Transcribe>> {
            let path_ref = custom_path.as_deref();
            match provider {
                Provider::LocalVoxtral => {
                    info!("Loading Voxtral model...");
                    VoxtralModel::new(&name, path_ref, cpu)
                        .map(|m| Box::new(m) as Box<dyn Transcribe>)
                }
                Provider::LocalWhisper => {
                    info!("Loading Whisper model...");
                    WhisperModel::new(&name, path_ref, cpu)
                        .map(|m| Box::new(m) as Box<dyn Transcribe>)
                }
                Provider::Online(_) => {
                    anyhow::bail!("load_model_sync called with online provider {provider}")
                }
            }
        };

        Self::load_with_fallback(load, force_cpu)
    }

    /// Try `load(force_cpu)`, and if CUDA fails, retry on CPU.
    fn load_with_fallback<F>(load: F, force_cpu: bool) -> Result<Box<dyn Transcribe>>
    where
        F: Fn(bool) -> Result<Box<dyn Transcribe>>,
    {
        match load(force_cpu) {
            Ok(instance) => Ok(instance),
            Err(e) if !force_cpu => {
                warn!("Failed to load model on CUDA: {e}. Attempting CPU fallback...");
                load(true).map_err(|cpu_e| {
                    error!("Both CUDA and CPU loading failed. CUDA error: {e}, CPU error: {cpu_e}");
                    cpu_e
                })
            }
            Err(e) => {
                error!("Model loading failed: {e}");
                Err(e)
            }
        }
    }

    /// Pre-flight VRAM check for built-in models.
    fn preflight_vram_check(
        def: &super_stt_shared::models::registry::ModelDefinition,
        force_cpu: bool,
    ) -> Result<()> {
        if force_cpu {
            return Ok(());
        }
        let required = def.estimated_vram_bytes;
        if required == 0 {
            return Ok(());
        }
        match Self::get_cuda_free_memory() {
            Ok(free) => {
                if free < required {
                    #[allow(clippy::cast_precision_loss)]
                    let free_gb = free as f64 / 1_073_741_824.0;
                    #[allow(clippy::cast_precision_loss)]
                    let required_gb = required as f64 / 1_073_741_824.0;
                    error!(
                        "Insufficient GPU memory for {}: {free_gb:.1} GB free, \
                         {required_gb:.1} GB required",
                        def.name
                    );
                    anyhow::bail!(
                        "Not enough GPU memory to load {}. \
                         {free_gb:.1} GB free, {required_gb:.1} GB required. \
                         Try a smaller model or switch to CPU.",
                        def.name
                    );
                }
                let free_mb = free / (1024 * 1024);
                info!("GPU memory check passed: {free_mb} MB free");
            }
            Err(e) => {
                info!("Could not query GPU memory ({e}), proceeding with CUDA attempt");
            }
        }
        Ok(())
    }

    /// Query free CUDA GPU memory in bytes.
    #[cfg(feature = "cuda")]
    fn get_cuda_free_memory() -> Result<u64> {
        use candle_core::cuda_backend::cudarc::driver::{result, safe::CudaContext};
        let _ctx =
            CudaContext::new(0).map_err(|e| anyhow::anyhow!("CUDA context init failed: {e}"))?;
        let (free, _total) =
            result::mem_get_info().map_err(|e| anyhow::anyhow!("CUDA mem_get_info: {e}"))?;
        Ok(free as u64)
    }

    #[cfg(not(feature = "cuda"))]
    fn get_cuda_free_memory() -> Result<u64> {
        Err(anyhow::anyhow!("CUDA not available"))
    }

    /// Download and load a built-in model.
    ///
    /// # Errors
    /// This function will return an error if the model fails to download or load.
    pub async fn download_and_load_model(
        &self,
        model: String,
        provider: Provider,
        tracker: Arc<DownloadProgressTracker>,
        start_time: std::time::Instant,
    ) -> anyhow::Result<Box<dyn Transcribe>> {
        let def = registry::find_by(&model, provider).ok_or_else(|| {
            anyhow::anyhow!(
                "download_and_load_model called with non-built-in '{model}' via {provider}; \
                 custom models load directly without download"
            )
        })?;

        // Download model files if not already cached
        crate::stt_models::local::download::with_progress(def, Arc::clone(&tracker)).await?;

        if tracker.is_cancelled() {
            anyhow::bail!("Model loading was cancelled");
        }
        *tracker.status.write() = "loading_model".to_string();
        *tracker.current_file.write() = "Loading model into memory...".to_string();
        tracker.broadcast_progress().await;

        let preferred_device = self.preferred_device.read().await.clone();
        let preferred_device_for_check = preferred_device.clone();
        let name = model.clone();
        let instance = tokio::task::spawn_blocking(move || {
            let result = Self::load_model_sync(name, provider, None, &preferred_device);
            let duration = start_time.elapsed();
            info!("Model loading completed in {duration:?}");
            result
        })
        .await??;

        // Update actual device based on what was loaded
        let actual_device_str = match instance.device() {
            candle_core::Device::Cpu => "cpu",
            candle_core::Device::Cuda(_) => "cuda",
            candle_core::Device::Metal(_) => "metal",
        };
        *self.actual_device.write().await = actual_device_str.to_string();

        if actual_device_str != preferred_device_for_check && preferred_device_for_check == "cuda" {
            warn!("CUDA loading failed, successfully fell back to CPU");
            info!("Model loaded on CPU fallback device");
        } else {
            info!("Model loaded successfully on {actual_device_str} device");
        }

        Ok(instance)
    }

    async fn finalize_model_switch_success(
        &self,
        model: String,
        provider: Provider,
        instance: Box<dyn Transcribe>,
        tracker: &Arc<DownloadProgressTracker>,
    ) -> DaemonResponse {
        tracker.mark_completed();
        *tracker.current_file.write() = "Model loaded successfully".to_string();
        tracker.broadcast_progress().await;
        self.download_manager.clear_download();
        let definition = registry::find_by(&model, provider)
            .cloned()
            .expect("built-in switch resolved a registry entry");
        *self.model.write().await = Some(crate::daemon::types::LoadedModel {
            definition,
            instance,
        });
        {
            let mut config_guard = self.config.write().await;
            config_guard.update_preferred_model(model.clone(), provider, SourceKind::Builtin);
        }
        if let Err(e) = self.broadcast_config_change().await {
            warn!("Failed to broadcast config change after model switch: {e}");
        }
        let _ = self
            .notification_manager
            .broadcast_event(
                "daemon_status_changed".to_string(),
                "daemon".to_string(),
                serde_json::json!({
                    "status": "model_switched",
                    "model_name": model.clone(),
                    "timestamp": Utc::now().to_rfc3339()
                }),
            )
            .await;
        let _ = self
            .notification_manager
            .broadcast_event(
                "daemon_status_changed".to_string(),
                "daemon".to_string(),
                serde_json::json!({
                    "status": "ready",
                    "model_loaded": true,
                    "model_name": model.clone(),
                    "timestamp": Utc::now().to_rfc3339()
                }),
            )
            .await;
        DaemonResponse::success()
            .with_current_model(model.clone())
            .with_current_provider(provider)
            .with_message(format!("Successfully switched to model: {model}"))
    }
}
