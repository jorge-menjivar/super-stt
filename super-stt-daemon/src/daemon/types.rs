// SPDX-License-Identifier: GPL-3.0-only
use crate::config::DaemonConfig;
use crate::daemon::events::EventBus;
use crate::download_progress::DownloadStateManager;
use crate::input::audio::AudioProcessor;
use crate::services::dbus::DBusManager;
use crate::services::transcription::RealTimeTranscriptionManager;
use crate::stt_models::local::download::CustomModelInfo;
use crate::stt_models::third_party::{
    deepgram::DeepgramModel, mistralai::MistralModel, openai::OpenAIModel,
};
use anyhow::Result;
use log::{info, warn};
use std::sync::{Arc, RwLock};
use super_stt_shared::models::provider::{OnlineProvider, Provider};
use super_stt_shared::models::registry;
use super_stt_shared::resource_management::ResourceManager;
use super_stt_shared::theme::AudioTheme;
use tokio::sync::broadcast;

#[derive(Copy, Clone, Debug)]
pub enum DeviceOverride {
    Cpu,
    Cuda,
}

/// Map a candle `Device` to the short wire-name (`"cpu"` / `"cuda"` /
/// `"metal"`) used in `daemon_status_changed` SSE payloads and on the
/// `/active_device` endpoint. Mirrors the same mapping inlined at
/// other call sites (e.g. `device_management::handle_device_switch_success`).
#[must_use]
pub(crate) fn device_str(device: &candle_core::Device) -> &'static str {
    match device {
        candle_core::Device::Cpu => "cpu",
        candle_core::Device::Cuda(_) => "cuda",
        candle_core::Device::Metal(_) => "metal",
    }
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
    pub model: SharedLoadedModel,
    pub audio_processor: Arc<AudioProcessor>,
    pub shutdown_tx: broadcast::Sender<()>,
    pub dbus_manager: Option<Arc<DBusManager>>,
    pub realtime_manager: Arc<RealTimeTranscriptionManager>,
    /// Internal pub/sub bus that fans recording / audio / STT events
    /// out to widget HTTP/SSE subscribers via `GET /events`.
    pub events: Arc<EventBus>,
    pub audio_theme: Arc<RwLock<AudioTheme>>,
    pub volume: Arc<RwLock<u8>>,
    pub is_recording: Arc<tokio::sync::RwLock<bool>>,
    pub download_manager: Arc<DownloadStateManager>,
    // Device management
    pub preferred_device: Arc<tokio::sync::RwLock<String>>, // "cpu" or "cuda"
    pub actual_device: Arc<tokio::sync::RwLock<String>>,    // actual device in use (may fallback)
    // Configuration management
    pub config: Arc<tokio::sync::RwLock<DaemonConfig>>,
    // Resource management for connection and rate limiting
    pub resource_manager: Arc<ResourceManager>,
    // Preview typing setting (beta feature)
    pub preview_typing_enabled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    // Sender used to signal a running recording to stop early (shortcut or external stop)
    pub manual_stop_tx: Arc<tokio::sync::RwLock<Option<tokio::sync::broadcast::Sender<()>>>>,
    // Cached keyboard simulator (session persists across recordings)
    pub simulator: Arc<tokio::sync::RwLock<Option<crate::output::keyboard::Simulator>>>,
    // Channel for streaming preview text to a waiting client (set by the recording flow)
    pub preview_text: Arc<tokio::sync::RwLock<Option<tokio::sync::mpsc::UnboundedSender<String>>>>,
    // Custom models discovered from custom_models_dir
    pub custom_models: Arc<tokio::sync::RwLock<Vec<CustomModelInfo>>>,
}

impl SuperSTTDaemon {
    /// Create a new `SuperSTTDaemon` instance
    ///
    /// # Errors
    ///
    /// Returns an error if model loading fails.
    #[allow(clippy::too_many_lines)]
    pub async fn new(
        stt_model_override: Option<String>,
        device_override: Option<DeviceOverride>,
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
        let audio_processor = Arc::new(AudioProcessor::new());

        // Initialize model storage (None until the initial load below succeeds)
        let model: SharedLoadedModel = Arc::new(tokio::sync::RwLock::new(None));

        // Initialize other managers
        let realtime_manager = Arc::new(RealTimeTranscriptionManager::new(
            Arc::clone(&model),
            Arc::clone(&audio_processor),
        ));
        let download_manager = Arc::new(DownloadStateManager::new());

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
            model,
            audio_processor,
            shutdown_tx,
            dbus_manager,
            realtime_manager,
            events: Arc::new(EventBus::new()),
            audio_theme: Arc::new(RwLock::new(config.audio.theme)),
            volume: Arc::new(RwLock::new(config.audio.volume)),
            is_recording: Arc::new(tokio::sync::RwLock::new(false)),
            download_manager,
            preferred_device: Arc::new(tokio::sync::RwLock::new(preferred_device)),
            actual_device: Arc::new(tokio::sync::RwLock::new(actual_device)),
            config: Arc::new(tokio::sync::RwLock::new(config)),
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
        Self::broadcast_loading_status(&daemon.events);

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
            let default_source = default_def.source.kind();
            if model_to_load == default && provider_to_load == default_provider {
                return Err(e);
            }
            warn!(
                "Failed to load preferred model {model_to_load} ({provider_to_load}): {e}; \
                 falling back to {default} ({default_provider})"
            );
            daemon.download_manager.clear_download();
            Self::load_initial_model_and_broadcast(&daemon, default.clone(), default_provider)
                .await?;
            // Persist the fallback so subsequent startups don't repeat the
            // failed attempt and the user doesn't see the same error again.
            daemon.config.write().await.update_preferred_model(
                default,
                default_provider,
                default_source,
            );
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

    fn broadcast_loading_status(events: &EventBus) {
        events.publish_daemon_status_changed(serde_json::json!({
            "status": "loading_model",
            "timestamp": chrono::Utc::now().to_rfc3339(),
        }));
    }

    async fn load_initial_model_and_broadcast(
        daemon: &SuperSTTDaemon,
        model_to_load: String,
        provider: Provider,
    ) -> Result<()> {
        daemon.broadcast_model_loading_status(&model_to_load);

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
            // Capture the device string before moving `instance` into the
            // shared `LoadedModel` — the `ready` event consumers (e.g. the
            // settings app's current_device tracking) only update their UI
            // when `actual_device` is present in the payload.
            let actual_device = device_str(instance.device());
            *daemon.model.write().await = Some(LoadedModel {
                definition,
                instance,
            });

            daemon
                .events
                .publish_daemon_status_changed(serde_json::json!({
                    "status": "ready",
                    "model_loaded": true,
                    "provider": provider.to_string(),
                    "actual_device": actual_device,
                    "model_name": model_to_load,
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                }));
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
        tracker.broadcast_progress();
        daemon.download_manager.clear_download();

        // Store into daemon state
        info!("{provider} model loaded successfully");
        let definition = registry::find_by(&model_to_load, provider)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("{model_to_load} via {provider}: not in registry"))?;
        // Capture the device string before moving `instance` into the
        // shared `LoadedModel` — the `ready` event consumers (e.g. the
        // settings app's current_device tracking) only update their UI
        // when `actual_device` is present in the payload.
        let actual_device = device_str(instance.device());
        *daemon.model.write().await = Some(LoadedModel {
            definition,
            instance,
        });

        // Broadcast ready status
        daemon
            .events
            .publish_daemon_status_changed(serde_json::json!({
                "status": "ready",
                "model_loaded": true,
                "actual_device": actual_device,
                "model_name": model_to_load,
                "timestamp": chrono::Utc::now().to_rfc3339(),
            }));
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

    /// Publish a recording-state transition on the SSE event bus so any
    /// connected `/events` subscribers (e.g. the COSMIC applet) update
    /// their visualization. The legacy notification path is gone; this
    /// is the only fan-out.
    pub fn broadcast_recording_state_change(&self, is_recording: bool) {
        self.events.publish_recording_state(is_recording);
    }

    /// Persist the current config to disk. Settings handlers call this
    /// after mutating the in-memory config so the change survives a
    /// restart. The legacy `config_changed` broadcast that used to
    /// follow this save is no longer part of the documented protocol
    /// (see `docs/protocol/endpoints/v1/events.md`); a future
    /// cross-app sync mechanism should be added as a documented topic.
    ///
    /// # Errors
    ///
    /// Returns an error if the on-disk write fails.
    pub async fn persist_config(&self) -> Result<(), anyhow::Error> {
        Self::persist_config_static(&self.config).await
    }

    /// Static variant of [`persist_config`] for use in spawned tasks
    /// that hold a `Clone<Arc<RwLock<DaemonConfig>>>` directly.
    ///
    /// # Errors
    ///
    /// Returns an error if the on-disk write fails.
    pub async fn persist_config_static(
        config: &Arc<tokio::sync::RwLock<DaemonConfig>>,
    ) -> Result<(), anyhow::Error> {
        let config_guard = config.read().await;
        config_guard
            .save()
            .map_err(|e| anyhow::anyhow!("Failed to save config to disk: {e}"))?;
        log::debug!("Persisted config to disk");
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
