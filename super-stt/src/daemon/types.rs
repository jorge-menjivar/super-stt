// SPDX-License-Identifier: GPL-3.0-only
use crate::audio::streamer::UdpAudioStreamer;
use crate::config::DaemonConfig;
use crate::daemon::auth::ProcessAuth;
use crate::download_progress::DownloadStateManager;
use crate::input::audio::AudioProcessor;
use crate::services::dbus::DBusManager;
use crate::services::transcription::RealTimeTranscriptionManager;
use crate::stt_models::local::download::CustomModelInfo;
use crate::stt_models::third_party::{
    deepgram::DeepgramModel, mistralai::MistralModel, openai::OpenAIModel,
};
use anyhow::{Context, Result};
use log::{info, warn};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use super_stt_shared::NotificationManager;
use super_stt_shared::models::provider::{OnlineProvider, Provider};
use super_stt_shared::models::registry;
use super_stt_shared::resource_management::ResourceManager;
use super_stt_shared::theme::AudioTheme;
use tokio::net::UnixListener;
use tokio::sync::broadcast;

use super::client_management::ClientConnectionsMap;

#[derive(Copy, Clone, Debug)]
pub enum DeviceOverride {
    Cpu,
    Cuda,
}

/// A live model: its full resolved [`ModelDefinition`] plus the running
/// inference instance. Replaces what used to be parallel
/// `Arc<RwLock<Option<…>>>` slots for name, provider, and instance — those
/// always changed together and could drift during a switch. The definition
/// owns the name, provider, source, and architecture; nothing has to be
/// re-derived at read sites.
pub struct LoadedModel {
    pub definition: super_stt_shared::models::registry::ModelDefinition,
    pub instance: Box<dyn crate::stt_models::transcribe::Transcribe>,
}

/// Shared handle to the currently-loaded model (or `None` while idle/loading).
pub type SharedLoadedModel = Arc<tokio::sync::RwLock<Option<LoadedModel>>>;

#[derive(Clone)]
pub struct SuperSTTDaemon {
    pub socket_path: PathBuf,
    pub model: SharedLoadedModel,
    pub notification_manager: Arc<NotificationManager>,
    pub audio_processor: Arc<AudioProcessor>,
    pub shutdown_tx: broadcast::Sender<()>,
    pub dbus_manager: Option<Arc<DBusManager>>,
    pub realtime_manager: Arc<RealTimeTranscriptionManager>,
    pub udp_streamer: Arc<UdpAudioStreamer>,
    pub audio_theme: Arc<RwLock<AudioTheme>>,
    pub volume: Arc<RwLock<u8>>,
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
    // Sender used to signal a running recording to stop early (shortcut or external stop)
    pub manual_stop_tx: Arc<tokio::sync::RwLock<Option<tokio::sync::broadcast::Sender<()>>>>,
    // Cached keyboard simulator (session persists across recordings)
    pub simulator: Arc<tokio::sync::RwLock<Option<crate::output::keyboard::Simulator>>>,
    // Channel for streaming preview text to a waiting client (set by client_management)
    pub preview_text: Arc<tokio::sync::RwLock<Option<tokio::sync::mpsc::UnboundedSender<String>>>>,
    // Custom models discovered from custom_models_dir
    pub custom_models: Arc<tokio::sync::RwLock<Vec<CustomModelInfo>>>,
}

impl SuperSTTDaemon {
    /// Create a new `SuperSTTDaemon` instance
    ///
    /// # Errors
    ///
    /// Returns an error if initializing subsystems (like UDP streamer) fails
    /// or if model loading fails.
    #[allow(clippy::too_many_lines)]
    pub async fn new(
        socket_path: PathBuf,
        stt_model_override: Option<String>,
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

        // Initialize model storage (None until the initial load below succeeds)
        let model: SharedLoadedModel = Arc::new(tokio::sync::RwLock::new(None));

        // Initialize other managers
        let realtime_manager = Arc::new(RealTimeTranscriptionManager::new(
            Arc::clone(&model),
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
            notification_manager,
            audio_processor,
            shutdown_tx,
            dbus_manager,
            realtime_manager,
            udp_streamer,
            audio_theme: Arc::new(RwLock::new(config.audio.theme)),
            volume: Arc::new(RwLock::new(config.audio.volume)),
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
            manual_stop_tx: Arc::new(tokio::sync::RwLock::new(None)),
            simulator: Arc::new(tokio::sync::RwLock::new(None)),
            preview_text: Arc::new(tokio::sync::RwLock::new(None)),
            custom_models: Arc::new(tokio::sync::RwLock::new(Vec::new())),
        };

        // Scan custom models directory if configured
        daemon.refresh_custom_models().await;

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

        // Load the appropriate STT model based on config preferences.
        // (name, provider) is the durable identity, both stored in config.
        // If the preferred model is online but online models are disabled
        // or no API key is present, fall back to the safe default.
        let (model_to_load, provider_to_load) = {
            let config_guard = daemon.config.read().await;
            let preferred = config_guard.transcription.preferred_model.clone();
            let preferred_provider = config_guard.transcription.preferred_provider;
            if let Provider::Online(online) = preferred_provider {
                if config_guard.online.allow_online_models
                    && crate::keyring::has_api_key(online.api_key_name()).unwrap_or(false)
                {
                    (preferred, preferred_provider)
                } else {
                    warn!(
                        "Preferred model is online but online models are disabled or no API key; falling back to default"
                    );
                    let default = registry::default_definition();
                    (default.name.to_string(), default.provider)
                }
            } else {
                (preferred, preferred_provider)
            }
        };

        // Try the preferred model; on failure, fall back to the safe default so the
        // daemon comes up in a usable state instead of exiting (e.g. configured
        // model name no longer exists in the registry).
        if let Err(e) =
            Self::load_initial_model_and_broadcast(&daemon, model_to_load.clone(), provider_to_load)
                .await
        {
            let default_def = registry::default_definition();
            let default = default_def.name.to_string();
            let default_provider = default_def.provider;
            if model_to_load == default && provider_to_load == default_provider {
                return Err(e);
            }
            warn!(
                "Failed to load preferred model {model_to_load} ({provider_to_load}): {e}; \
                 falling back to {default} ({default_provider})"
            );
            daemon.download_manager.clear_download();
            Self::load_initial_model_and_broadcast(&daemon, default, default_provider).await?;
        }

        Ok(daemon)
    }

    fn apply_cli_overrides_to_config(
        config: &mut DaemonConfig,
        stt_model_override: Option<String>,
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
        model_to_load: String,
        provider: Provider,
    ) -> Result<()> {
        daemon
            .broadcast_model_loading_status(model_to_load.clone())
            .await;

        // Online providers don't need downloading — create instance directly
        if let Provider::Online(online) = provider {
            let key_name = online.api_key_name();
            let api_key = crate::keyring::get_api_key(key_name)
                .map_err(|e| anyhow::anyhow!(e))?
                .ok_or_else(|| anyhow::anyhow!("{key_name} API key not configured"))?;

            let instance = Self::create_online_instance(online, api_key, &model_to_load)
                .map_err(|e| anyhow::anyhow!(e))?;
            info!("{provider} model {model_to_load} loaded successfully");
            let definition = registry::find_by(&model_to_load, provider)
                .cloned()
                .ok_or_else(|| {
                    anyhow::anyhow!("{model_to_load} via {provider}: not in registry")
                })?;
            *daemon.model.write().await = Some(LoadedModel {
                definition,
                instance,
            });

            if let Err(e) = daemon
                .notification_manager
                .broadcast_event(
                    "daemon_status_changed".to_string(),
                    "daemon".to_string(),
                    serde_json::json!({
                        "status": "ready",
                        "model_loaded": true,
                        "provider": provider.to_string(),
                        "timestamp": chrono::Utc::now().to_rfc3339()
                    }),
                )
                .await
            {
                warn!("Failed to broadcast model ready status: {e}");
            }
            return Ok(());
        }

        // Local model path: download if needed, then load
        let tracker = daemon.create_progress_tracker(&model_to_load);
        if let Err(resp) = daemon.register_download(&tracker) {
            tracker.cancel();
            anyhow::bail!(
                resp.message
                    .unwrap_or_else(|| "Failed to register download".to_string())
            );
        }

        let start_time = std::time::Instant::now();
        let instance = daemon
            .download_and_load_model(
                model_to_load.clone(),
                provider,
                Arc::clone(&tracker),
                start_time,
            )
            .await?;

        // Mark completed and clear download state
        tracker.mark_completed();
        *tracker.current_file.write() = "Model loaded successfully".to_string();
        tracker.broadcast_progress().await;
        daemon.download_manager.clear_download();

        // Store into daemon state
        info!("{provider} model loaded successfully");
        let definition = registry::find_by(&model_to_load, provider)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("{model_to_load} via {provider}: not in registry"))?;
        *daemon.model.write().await = Some(LoadedModel {
            definition,
            instance,
        });

        // Broadcast ready status
        if let Err(e) = daemon
            .notification_manager
            .broadcast_event(
                "daemon_status_changed".to_string(),
                "daemon".to_string(),
                serde_json::json!({
                    "status": "ready",
                    "model_loaded": true,
                    "timestamp": chrono::Utc::now().to_rfc3339()
                }),
            )
            .await
        {
            warn!("Failed to broadcast model ready status: {e}");
        }
        Ok(())
    }

    /// Create the appropriate online model instance based on provider.
    ///
    /// # Errors
    /// Returns an error if `name` is unknown to the provider's registry entries
    /// or if constructing the underlying client fails (e.g. malformed API key).
    pub fn create_online_instance(
        provider: OnlineProvider,
        api_key: String,
        name: &str,
    ) -> Result<Box<dyn crate::stt_models::transcribe::Transcribe>> {
        Ok(match provider {
            OnlineProvider::OpenAI => Box::new(OpenAIModel::new(name, api_key)?),
            OnlineProvider::Mistral => Box::new(MistralModel::new(name, api_key)?),
            OnlineProvider::Deepgram => Box::new(DeepgramModel::new(name, api_key)?),
        })
    }

    /// Resolve a wire-level `(name, provider, source)` triple into a
    /// [`ModelDefinition`], consulting both the static built-in registry
    /// and the daemon's discovered custom models. Returns `None` on miss.
    pub async fn resolve_definition(
        &self,
        name: &str,
        provider: Provider,
        source: super_stt_shared::models::registry::SourceKind,
    ) -> Option<super_stt_shared::models::registry::ModelDefinition> {
        let custom = self.custom_models.read().await;
        super_stt_shared::models::registry::resolve(name, provider, source, &custom)
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

    /// Set the master volume (0-100)
    pub fn set_volume(&self, volume: u8) {
        match self.volume.write() {
            Ok(mut guard) => {
                *guard = volume;
                log::info!("Volume changed to: {volume}");
            }
            Err(poisoned) => {
                log::warn!("Volume lock was poisoned, attempting recovery");
                let mut guard = poisoned.into_inner();
                *guard = volume;
                log::info!("Volume changed to: {volume} (after lock recovery)");
            }
        }
    }

    /// Get the current master volume (0-100)
    #[must_use]
    pub fn get_volume(&self) -> u8 {
        match self.volume.read() {
            Ok(guard) => *guard,
            Err(poisoned) => {
                log::warn!("Volume lock was poisoned, returning current value");
                *poisoned.into_inner()
            }
        }
    }

    /// Get the current volume as a f32 multiplier (0.0-1.0)
    #[must_use]
    pub fn get_volume_f32(&self) -> f32 {
        f32::from(self.get_volume()) / 100.0
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_online_instance_openai() {
        let instance = SuperSTTDaemon::create_online_instance(
            OnlineProvider::OpenAI,
            "test-key".to_string(),
            "whisper-1",
        )
        .unwrap();
        assert!(instance.is_online());
    }

    #[test]
    fn create_online_instance_mistral() {
        let instance = SuperSTTDaemon::create_online_instance(
            OnlineProvider::Mistral,
            "test-key".to_string(),
            "voxtral-mini-latest",
        )
        .unwrap();
        assert!(instance.is_online());
    }

    #[test]
    fn create_online_instance_deepgram() {
        let instance = SuperSTTDaemon::create_online_instance(
            OnlineProvider::Deepgram,
            "test-key".to_string(),
            "nova-3",
        )
        .unwrap();
        assert!(instance.is_online());
    }

    #[test]
    fn online_instance_device_returns_cpu() {
        let instance = SuperSTTDaemon::create_online_instance(
            OnlineProvider::OpenAI,
            "key".to_string(),
            "whisper-1",
        )
        .unwrap();
        assert!(matches!(instance.device(), candle_core::Device::Cpu));
    }
}
