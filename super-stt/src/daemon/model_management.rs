// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::{STTModelInstance, SuperSTTDaemon};
use crate::download_progress::DownloadProgressTracker;
use crate::stt_models::{voxtral::VoxtralModel, whisper::WhisperModel};
use anyhow::Result;
use chrono::Utc;
use log::{error, info, warn};
use std::sync::Arc;
use super_stt_shared::models::protocol::DaemonResponse;
use super_stt_shared::stt_model::STTModel;

impl SuperSTTDaemon {
    /// Load model with explicit target device (used during device switching)
    ///
    /// # Errors
    ///
    /// Returns an error if model loading fails on both the requested device
    /// and any attempted fallback.
    pub async fn load_model_with_target_device(
        &self,
        stt_model: &STTModel,
        target_device: &str,
    ) -> Result<STTModelInstance> {
        let stt_model_copy = *stt_model;
        let target_device_copy = target_device.to_string();
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        info!("Loading model with target device: {target_device}");

        // Load model in a single blocking task with cancellation support
        let load_handle = tokio::task::spawn_blocking(move || {
            Self::load_model_sync(stt_model_copy, &target_device_copy)
        });

        // Wait for either model loading completion, shutdown signal, or timeout (60 seconds)
        let model_result = tokio::select! {
            result = load_handle => {
                result.map_err(|e| anyhow::anyhow!("Model loading task failed: {}", e))?
            }
            _ = shutdown_rx.recv() => {
                warn!("Model loading cancelled due to shutdown");
                return Err(anyhow::anyhow!("Model loading cancelled due to shutdown"));
            }
            () = tokio::time::sleep(tokio::time::Duration::from_secs(60)) => {
                error!("Model loading timed out after 60 seconds");
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
        stt_model: &STTModel,
    ) -> Result<STTModelInstance> {
        let preferred_device = self.preferred_device.read().await.clone();
        self.load_model_with_target_device(stt_model, &preferred_device)
            .await
    }

    /// Handle get current model command
    pub async fn handle_get_model(&self) -> DaemonResponse {
        let model_type_guard = self.model_type.read().await;

        if let Some(model) = model_type_guard.as_ref() {
            info!("Current model requested: {model}");
            DaemonResponse::success()
                .with_current_model(*model)
                .with_message(format!("Current model: {model}"))
        } else {
            warn!("No model is currently loaded");
            DaemonResponse::error("No model is currently loaded")
        }
    }

    /// Handle set model command - switch to a different model
    pub async fn handle_set_model(&self, model: STTModel) -> DaemonResponse {
        self.handle_set_model_impl(model).await
    }

    /// Internal implementation for model switching (split to reduce public fn size)
    async fn handle_set_model_impl(&self, model: STTModel) -> DaemonResponse {
        info!("Model switch requested: {model}");
        if let Some(resp) = self.preflight_model_switch(model).await {
            return resp;
        }
        self.broadcast_model_loading_status(model).await;
        let tracker = self.create_progress_tracker(model);
        if let Err(resp) = self.register_download(&tracker) {
            tracker.cancel();
            return *resp;
        }
        self.unload_current_model().await;
        let start_time = std::time::Instant::now();
        match self
            .download_and_load_model(model, Arc::clone(&tracker), start_time)
            .await
        {
            Ok(instance) => {
                self.finalize_model_switch_success(model, instance, &tracker)
                    .await
            }
            Err(e) => {
                error!("Model switch failed: {e}");
                self.download_manager.clear_download();
                DaemonResponse::error(&format!("Model switch failed: {e}"))
            }
        }
    }
}

impl SuperSTTDaemon {
    async fn preflight_model_switch(&self, model: STTModel) -> Option<DaemonResponse> {
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
        if let Some(current_model) = *self.model_type.read().await
            && current_model == model
        {
            info!("Model switch skipped - already using {model}");
            return Some(
                DaemonResponse::success()
                    .with_message(format!("Already using model: {model}"))
                    .with_current_model(current_model),
            );
        }

        None
    }

    pub async fn broadcast_model_loading_status(&self, model: STTModel) {
        if let Err(e) = self
            .notification_manager
            .broadcast_event(
                "daemon_status_changed".to_string(),
                "daemon".to_string(),
                serde_json::json!({
                    "status": "loading_model",
                    "new_model": model.to_string(),
                    "timestamp": Utc::now().to_rfc3339()
                }),
            )
            .await
        {
            warn!("Failed to broadcast model loading status: {e}");
        }
    }

    #[must_use]
    pub fn create_progress_tracker(&self, model: STTModel) -> Arc<DownloadProgressTracker> {
        let flag = self.download_manager.get_cancellation_flag();
        Arc::new(
            DownloadProgressTracker::new(model.to_string(), 0, Arc::clone(&flag))
                .with_notification_manager(Arc::clone(&self.notification_manager)),
        )
    }

    /// Register a download tracker with the download manager.
    ///
    /// # Errors
    /// This function will return an error if the download tracker fails to register.
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

    /// Synchronous model loading function that handles device preference and fallback
    /// This is the core blocking operation that should be run in `spawn_blocking`
    fn load_model_sync(model: STTModel, preferred_device: &str) -> Result<STTModelInstance> {
        let force_cpu = preferred_device == "cpu";
        info!("Loading model with device preference: {preferred_device} (force_cpu={force_cpu})");

        // Attempt to load model with preferred device
        let initial_result = match model {
            STTModel::VoxtralSmall | STTModel::VoxtralMini => {
                info!("Loading Voxtral model...");
                VoxtralModel::new(&model, force_cpu).map(|m| STTModelInstance::Voxtral(Box::new(m)))
            }
            _ => {
                info!("Loading Whisper model...");
                WhisperModel::new(&model, force_cpu).map(|m| STTModelInstance::Whisper(Box::new(m)))
            }
        };

        // Handle CUDA fallback if needed
        match initial_result {
            Ok(model_instance) => Ok(model_instance),
            Err(e) if !force_cpu => {
                // If CUDA failed, try CPU fallback
                warn!("Failed to load model on CUDA: {e}. Attempting CPU fallback...");

                match model {
                    STTModel::VoxtralSmall | STTModel::VoxtralMini => {
                        VoxtralModel::new(&model, true)
                            .map(|m| STTModelInstance::Voxtral(Box::new(m)))
                    }
                    _ => WhisperModel::new(&model, true)
                        .map(|m| STTModelInstance::Whisper(Box::new(m))),
                }
                .map_err(|cpu_e| {
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

    /// Download and load a model.
    ///
    /// # Errors
    /// This function will return an error if the model fails to download or load.
    pub async fn download_and_load_model(
        &self,
        model: STTModel,
        tracker: Arc<DownloadProgressTracker>,
        start_time: std::time::Instant,
    ) -> anyhow::Result<STTModelInstance> {
        crate::stt_models::download::with_progress(&model, Arc::clone(&tracker)).await?;
        if tracker.is_cancelled() {
            anyhow::bail!("Model loading was cancelled");
        }
        *tracker.status.write() = "loading_model".to_string();
        *tracker.current_file.write() = "Loading model into memory...".to_string();
        tracker.broadcast_progress().await;

        let preferred_device = self.preferred_device.read().await.clone();
        let preferred_device_clone = preferred_device.clone();
        let instance = tokio::task::spawn_blocking(move || {
            let result = Self::load_model_sync(model, &preferred_device_clone);
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

        if actual_device_str != preferred_device && preferred_device == "cuda" {
            warn!("CUDA loading failed, successfully fell back to CPU");
            info!("Model loaded on CPU fallback device");
        } else {
            info!("Model loaded successfully on {actual_device_str} device");
        }

        Ok(instance)
    }

    async fn finalize_model_switch_success(
        &self,
        model: STTModel,
        instance: STTModelInstance,
        tracker: &Arc<DownloadProgressTracker>,
    ) -> DaemonResponse {
        let model_name = match &instance {
            STTModelInstance::Whisper(_) => "Whisper",
            STTModelInstance::Voxtral(_) => "Voxtral",
        };
        tracker.mark_completed();
        *tracker.current_file.write() = "Model loaded successfully".to_string();
        tracker.broadcast_progress().await;
        self.download_manager.clear_download();
        {
            let mut model_guard = self.model.write().await;
            *model_guard = Some(instance);
        }
        {
            let mut model_type_guard = self.model_type.write().await;
            *model_type_guard = Some(model);
        }
        {
            let mut config_guard = self.config.write().await;
            config_guard.update_preferred_model(model);
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
                    "model_type": model_name.to_lowercase(),
                    "model_name": model.to_string(),
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
                    "model_type": model_name.to_lowercase(),
                    "model_name": model.to_string(),
                    "timestamp": Utc::now().to_rfc3339()
                }),
            )
            .await;
        DaemonResponse::success()
            .with_current_model(model)
            .with_message(format!("Successfully switched to model: {model}"))
    }
}
