// SPDX-License-Identifier: GPL-3.0-only
use crate::audio::streamer::UdpAudioStreamer;
use crate::config::DaemonConfig;
use crate::daemon::auth::ProcessAuth;
use crate::download_progress::DownloadStateManager;
use crate::input::audio::AudioProcessor;
use crate::services::dbus::DBusManager;
use crate::services::transcription::RealTimeTranscriptionManager;
use crate::stt_models::{voxtral::VoxtralModel, whisper::WhisperModel};
use anyhow::{Context, Result};
use log::{info, warn};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use super_stt_shared::NotificationManager;
use super_stt_shared::resource_management::ResourceManager;
use super_stt_shared::stt_model::STTModel;
use super_stt_shared::theme::AudioTheme;
use tokio::net::UnixListener;
use tokio::sync::broadcast;

use super::client_management::ClientConnectionsMap;

#[derive(Copy, Clone, Debug)]
pub enum DeviceOverride {
    Cpu,
    Cuda,
}

/// Enum to hold different STT model types
pub enum STTModelInstance {
    Whisper(Box<WhisperModel>),
    Voxtral(Box<VoxtralModel>),
}

impl STTModelInstance {
    /// Transcribe audio using the loaded model
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying model fails to transcribe.
    pub fn transcribe_audio(&mut self, audio_data: &[f32], sample_rate: u32) -> Result<String> {
        match self {
            STTModelInstance::Whisper(model) => model.transcribe_audio(audio_data, sample_rate),
            STTModelInstance::Voxtral(model) => model.transcribe_audio(audio_data, sample_rate),
        }
    }

    /// Get the device used by the model
    #[must_use]
    pub fn device(&self) -> &candle_core::Device {
        match self {
            STTModelInstance::Whisper(model) => model.device(),
            STTModelInstance::Voxtral(model) => model.device(),
        }
    }
}

#[derive(Clone)]
pub struct SuperSTTDaemon {
    pub socket_path: PathBuf,
    pub model: Arc<tokio::sync::RwLock<Option<STTModelInstance>>>,
    pub model_type: Arc<tokio::sync::RwLock<Option<super_stt_shared::stt_model::STTModel>>>,
    pub notification_manager: Arc<NotificationManager>,
    pub audio_processor: Arc<AudioProcessor>,
    pub shutdown_tx: broadcast::Sender<()>,
    pub dbus_manager: Option<Arc<DBusManager>>,
    pub realtime_manager: Arc<RealTimeTranscriptionManager>,
    pub udp_streamer: Arc<UdpAudioStreamer>,
    pub audio_theme: Arc<RwLock<AudioTheme>>,
    pub is_recording: Arc<tokio::sync::RwLock<bool>>,
    pub audio_monitoring_handle: Arc<tokio::sync::RwLock<Option<tokio::task::JoinHandle<()>>>>,
    pub download_manager: Arc<DownloadStateManager>,
    // Device management
    pub preferred_device: Arc<tokio::sync::RwLock<String>>, // "cpu" or "cuda"
    pub actual_device: Arc<tokio::sync::RwLock<String>>,    // actual device in use (may fallback)
    // Configuration management
    pub config: Arc<tokio::sync::RwLock<DaemonConfig>>,
    // Connection tracking
    pub active_connections: ClientConnectionsMap,
    // Process authentication for write operations
    pub process_auth: ProcessAuth,
    // Resource management for connection and rate limiting
    pub resource_manager: Arc<ResourceManager>,
    // Preview typing setting (beta feature)
    pub preview_typing_enabled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    // Mutex to prevent GPU processing during typing operations
}

impl SuperSTTDaemon {
    /// Create a new `SuperSTTDaemon` instance
    ///
    /// # Errors
    ///
    /// Returns an error if initializing subsystems (like UDP streamer) fails
    /// or if model loading fails.
    pub async fn new(
        socket_path: PathBuf,
        stt_model_override: Option<STTModel>,
        device_override: Option<DeviceOverride>,
        udp_port: u16,
        audio_theme_override: Option<AudioTheme>,
    ) -> Result<Self> {
        info!("Initializing Super STT Daemon...");

        // Load or initialize daemon configuration
        let mut config = DaemonConfig::load();
        info!("Loaded daemon configuration from disk");
        let config_changed = Self::apply_cli_overrides_to_config(
            &mut config,
            stt_model_override,
            device_override,
            audio_theme_override,
        );
        if config_changed {
            if let Err(e) = config.save() {
                warn!("Failed to save updated daemon config: {e}");
            } else {
                info!("Updated daemon configuration saved to disk");
            }
        }

        // Initialize components
        let (shutdown_tx, _) = broadcast::channel(1);
        let notification_manager = Arc::new(NotificationManager::new(1000, 100)); // max 1000 events, 100 subscribers
        let audio_processor = Arc::new(AudioProcessor::new());

        // Initialize model storage
        let model = Arc::new(tokio::sync::RwLock::new(None));
        let model_type = Arc::new(tokio::sync::RwLock::new(Some(
            config.transcription.preferred_model,
        )));

        // Initialize other managers
        let realtime_manager = Arc::new(RealTimeTranscriptionManager::new(
            Arc::clone(&model),
            Arc::clone(&model_type),
            Arc::clone(&notification_manager),
            Arc::clone(&audio_processor),
        ));
        let udp_bind_addr = format!("127.0.0.1:{udp_port}");
        let udp_streamer = {
            let streamer = Arc::new(UdpAudioStreamer::new(&udp_bind_addr).await?);
            info!("UDP audio streamer initialized on port {udp_port}");
            streamer.start_cleanup_task(&shutdown_tx);
            let _ = streamer.start_registration_listener(&shutdown_tx).await;
            streamer
        };

        let download_manager = Arc::new(DownloadStateManager::new());

        // Initialize process authentication for write operations
        let process_auth = ProcessAuth::new();

        // Initialize resource manager for connection and rate limiting
        let resource_manager = if cfg!(debug_assertions) {
            Arc::new(ResourceManager::development())
        } else {
            Arc::new(ResourceManager::production())
        };

        // Initialize D-Bus manager (optional, may fail on systems without D-Bus)
        let dbus_manager = match DBusManager::new().await {
            Ok(mgr) => Some(Arc::new(mgr)),
            Err(e) => {
                warn!("D-Bus initialization failed (this is normal on some systems): {e}");
                None
            }
        };

        // Initialize device state based on config
        let preferred_device = config.device.preferred_device.clone();
        let actual_device = preferred_device.clone(); // Will be updated when model loads

        // Extract preview typing setting before config gets moved
        let preview_typing_enabled = config.transcription.preview_typing_enabled;

        // Create the daemon instance first (needed for model loading)
        let daemon = SuperSTTDaemon {
            socket_path,
            model,
            model_type: Arc::new(tokio::sync::RwLock::new(Some(
                config.transcription.preferred_model,
            ))),
            notification_manager,
            audio_processor,
            shutdown_tx,
            dbus_manager,
            realtime_manager,
            udp_streamer,
            audio_theme: Arc::new(RwLock::new(config.audio.theme)),
            is_recording: Arc::new(tokio::sync::RwLock::new(false)),
            audio_monitoring_handle: Arc::new(tokio::sync::RwLock::new(None)),
            download_manager,
            preferred_device: Arc::new(tokio::sync::RwLock::new(preferred_device)),
            actual_device: Arc::new(tokio::sync::RwLock::new(actual_device)),
            config: Arc::new(tokio::sync::RwLock::new(config)),
            active_connections: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            process_auth,
            resource_manager,
            preview_typing_enabled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
                preview_typing_enabled,
            )),
        };

        // Apply temporary device override for current session (not saved to config)
        if matches!(device_override, Some(DeviceOverride::Cpu)) {
            let mut preferred_device_guard = daemon.preferred_device.write().await;
            if *preferred_device_guard != "cpu" {
                info!(
                    "Temporary session override: device preference {} -> cpu (not saved)",
                    *preferred_device_guard
                );
                *preferred_device_guard = "cpu".to_string();
            }
        }

        // Broadcast loading status
        Self::broadcast_loading_status(&daemon.notification_manager).await;

        // Load the appropriate STT model based on config preferences
        let model_to_load = {
            let config_guard = daemon.config.read().await;
            config_guard.transcription.preferred_model
        };
        Self::load_initial_model_and_broadcast(&daemon, model_to_load).await?;

        Ok(daemon)
    }

    fn apply_cli_overrides_to_config(
        config: &mut DaemonConfig,
        stt_model_override: Option<STTModel>,
        device_override: Option<DeviceOverride>,
        audio_theme_override: Option<AudioTheme>,
    ) -> bool {
        let mut changed = false;
        // Only override device preference if provided explicitly
        if let Some(dev) = device_override {
            let desired = match dev {
                DeviceOverride::Cpu => "cpu",
                DeviceOverride::Cuda => "cuda",
            };
            if config.device.preferred_device != desired {
                info!(
                    "CLI override: device preference {} -> {}",
                    config.device.preferred_device, desired
                );
                config.device.preferred_device = desired.to_string();
                changed = true;
            }
        }
        if let Some(theme) = audio_theme_override
            && config.audio.theme != theme
        {
            info!(
                "CLI override: audio theme {:?} -> {:?}",
                config.audio.theme, theme
            );
            config.audio.theme = theme;
            changed = true;
        }
        if let Some(model) = stt_model_override
            && config.transcription.preferred_model != model
        {
            info!(
                "CLI override: model {:?} -> {:?}",
                config.transcription.preferred_model, model
            );
            config.transcription.preferred_model = model;
            changed = true;
        }

        changed
    }

    async fn broadcast_loading_status(notification_manager: &Arc<NotificationManager>) {
        if let Err(e) = notification_manager
            .broadcast_event(
                "daemon_status_changed".to_string(),
                "daemon".to_string(),
                serde_json::json!({
                    "status": "loading_model",
                    "timestamp": chrono::Utc::now().to_rfc3339()
                }),
            )
            .await
        {
            warn!("Failed to broadcast daemon loading status: {e}");
        }
    }

    async fn load_initial_model_and_broadcast(
        daemon: &SuperSTTDaemon,
        model_to_load: STTModel,
    ) -> Result<()> {
        // Mirror the model switch path: broadcast loading, download if needed, then load
        daemon.broadcast_model_loading_status(model_to_load).await;
        let tracker = daemon.create_progress_tracker(model_to_load);
        if let Err(resp) = daemon.register_download(&tracker) {
            tracker.cancel();
            // Surface the error so startup fails clearly
            anyhow::bail!(
                resp.message
                    .unwrap_or_else(|| "Failed to register download".to_string())
            );
        }

        let start_time = std::time::Instant::now();
        let instance = daemon
            .download_and_load_model(model_to_load, Arc::clone(&tracker), start_time)
            .await?;

        // Mark completed and clear download state
        tracker.mark_completed();
        *tracker.current_file.write() = "Model loaded successfully".to_string();
        tracker.broadcast_progress().await;
        daemon.download_manager.clear_download();

        // Store into daemon state
        let model_name = match &instance {
            STTModelInstance::Whisper(_) => "Whisper",
            STTModelInstance::Voxtral(_) => "Voxtral",
        };
        info!("{model_name} model loaded successfully");
        *daemon.model.write().await = Some(instance);
        *daemon.model_type.write().await = Some(model_to_load);

        // Broadcast ready status
        if let Err(e) = daemon
            .notification_manager
            .broadcast_event(
                "daemon_status_changed".to_string(),
                "daemon".to_string(),
                serde_json::json!({
                    "status": "ready",
                    "model_loaded": true,
                    "model_type": model_name.to_lowercase(),
                    "timestamp": chrono::Utc::now().to_rfc3339()
                }),
            )
            .await
        {
            warn!("Failed to broadcast model ready status: {e}");
        }
        Ok(())
    }

    /// Start the daemon and listen for connections
    ///
    /// # Errors
    ///
    /// Returns an error if the socket directory cannot be created,
    /// if binding the Unix socket fails, or if setting permissions fails.
    pub async fn start(&self) -> Result<()> {
        info!(
            "Starting Super STT Daemon on socket: {}",
            self.socket_path.display()
        );

        // Create parent directory if it doesn't exist
        if let Some(parent) = self.socket_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .context("Failed to create socket directory")?;
        }

        // Remove existing socket file
        if self.socket_path.exists() {
            tokio::fs::remove_file(&self.socket_path)
                .await
                .context("Failed to remove existing socket file")?;
        }

        // Create Unix domain socket listener
        let listener =
            UnixListener::bind(&self.socket_path).context("Failed to bind Unix socket")?;

        // Set socket permissions based on environment
        // Production: 0o660 - owner read/write, group read/write (for 'stt' group members)
        // Development: 0o666 - world read/write (for convenience during development)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            // Use compile-time security model: debug builds vs release builds
            let mode = if cfg!(debug_assertions) {
                log::warn!("Debug build - socket permissions set to 0o666 (world accessible)");
                log::warn!("For production security, use release builds: cargo build --release");
                0o666
            } else {
                log::info!("Socket permissions set to 0o660 (owner + stt group access only)");
                log::info!("Ensure users are in the 'stt' group: sudo usermod -a -G stt $USER");
                log::info!("Authorized binaries: super-stt, stt wrapper");
                0o660
            };

            let permissions = std::fs::Permissions::from_mode(mode);
            std::fs::set_permissions(&self.socket_path, permissions)
                .context("Failed to set socket permissions")?;
        }

        info!("Daemon listening on socket: {}", self.socket_path.display());

        // Set up shutdown receiver
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        // Main server loop
        loop {
            tokio::select! {
                // Accept new connections
                result = listener.accept() => {
                    match result {
                        Ok((stream, _addr)) => {
                            let daemon_clone = self.clone();
                            let mut client_shutdown_rx = self.shutdown_tx.subscribe();
                            tokio::spawn(async move {
                                tokio::select! {
                                    result = daemon_clone.handle_client(stream) => {
                                        if let Err(e) = result {
                                            log::warn!("Error handling client: {e}");
                                        }
                                    }
                                    _ = client_shutdown_rx.recv() => {
                                        log::debug!("Client handler cancelled due to shutdown");
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            log::error!("Failed to accept connection: {e}");
                        }
                    }
                }

                // Handle shutdown signal
                _ = shutdown_rx.recv() => {
                    info!("Shutdown signal received");
                    break;
                }
            }
        }

        // Cleanup with timeout to prevent hanging
        let cleanup_result = tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
            if self.socket_path.exists() {
                let _ = tokio::fs::remove_file(&self.socket_path).await;
            }
        })
        .await;

        if cleanup_result.is_err() {
            log::warn!("Socket cleanup timed out, continuing shutdown");
        }

        info!("Daemon shutdown complete");
        Ok(())
    }

    /// Set the audio theme
    ///
    /// If the lock is poisoned, logs a warning and attempts to recover by creating a new lock.
    pub fn set_audio_theme(&self, theme: AudioTheme) {
        match self.audio_theme.write() {
            Ok(mut guard) => {
                *guard = theme;
                log::info!("Audio theme changed to: {theme}");
            }
            Err(poisoned) => {
                log::warn!("Audio theme lock was poisoned, attempting recovery");
                let mut guard = poisoned.into_inner();
                *guard = theme;
                log::info!("Audio theme changed to: {theme} (after lock recovery)");
            }
        }
    }

    /// Get the current audio theme
    ///
    /// If the lock is poisoned, logs a warning and returns the default theme.
    #[must_use]
    pub fn get_audio_theme(&self) -> AudioTheme {
        match self.audio_theme.read() {
            Ok(guard) => *guard,
            Err(poisoned) => {
                log::warn!("Audio theme lock was poisoned, returning current value");
                *poisoned.into_inner()
            }
        }
    }

    /// Broadcast config change event to all connected clients
    ///
    /// # Errors
    ///
    /// Returns an error if serialization or broadcasting fails.
    pub async fn broadcast_config_change(&self) -> Result<(), anyhow::Error> {
        Self::broadcast_config_change_static(&self.notification_manager, &self.config).await
    }

    /// Static helper method to broadcast config changes (for use in spawned tasks)
    ///
    /// # Errors
    ///
    /// Returns an error if serialization or broadcasting fails.
    pub async fn broadcast_config_change_static(
        notification_manager: &Arc<NotificationManager>,
        config: &Arc<tokio::sync::RwLock<DaemonConfig>>,
    ) -> Result<(), anyhow::Error> {
        // Save config to disk first
        {
            let config_guard = config.read().await;
            if let Err(e) = config_guard.save() {
                log::warn!("Failed to save config to disk: {e}");
                return Err(anyhow::anyhow!("Failed to save config to disk: {e}"));
            }
        }

        // Then broadcast the change
        let config_guard = config.read().await;
        let config_json = serde_json::to_value(&*config_guard)?;
        drop(config_guard);

        notification_manager
            .broadcast_event(
                "config_changed".to_string(),
                "daemon".to_string(),
                serde_json::json!({
                    "config": config_json,
                    "timestamp": chrono::Utc::now().to_rfc3339()
                }),
            )
            .await?;

        log::debug!(
            "Saved config to disk and broadcasted config change event to all connected clients"
        );
        Ok(())
    }
}
