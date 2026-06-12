// SPDX-License-Identifier: GPL-3.0-only
use crate::config::DaemonConfig;
use crate::daemon::events::EventBus;
use crate::download_progress::DownloadStateManager;
use crate::input::audio::AudioProcessor;
use crate::services::dbus::DBusManager;
use crate::services::transcription::RealTimeTranscriptionManager;
use crate::stt_models::backends::{self, DiscoveredBackend};
use anyhow::Result;
use log::{info, warn};
use std::sync::{Arc, RwLock};
use super_stt_shared::models::provider::Provider;
use super_stt_shared::resource_management::ResourceManager;
use super_stt_shared::theme::AudioTheme;
use tokio::sync::broadcast;

#[derive(Copy, Clone, Debug)]
pub enum DeviceOverride {
    Cpu,
    Cuda,
}

/// Normalize a backend-reported device label to the short wire-name
/// (`"cpu"` / `"cuda"` / `"metal"` / `"remote"`) used in
/// `daemon_status_changed` SSE payloads and on the `/active_device` endpoint.
#[must_use]
pub(crate) fn normalize_device(label: &str) -> String {
    let l = label.to_ascii_lowercase();
    if l.contains("cuda") {
        "cuda".to_string()
    } else if l.contains("metal") {
        "metal".to_string()
    } else if l.contains("remote") {
        "remote".to_string()
    } else {
        "cpu".to_string()
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
    pub busy: Arc<tokio::sync::RwLock<bool>>,
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
    // Backends discovered from the backends directory.
    pub backends: Arc<tokio::sync::RwLock<Vec<DiscoveredBackend>>>,
    // Active backend: the relative install dir (subdir of the backends dir) of
    // the selected provider, or None when idle. Runtime mirror of
    // `config.transcription.active_backend`.
    pub active_backend: Arc<tokio::sync::RwLock<Option<String>>>,
}

/// Pre-assembled subsystem handles created during daemon startup.
struct DaemonComponents {
    shutdown_tx: broadcast::Sender<()>,
    audio_processor: Arc<AudioProcessor>,
    model: SharedLoadedModel,
    realtime_manager: Arc<RealTimeTranscriptionManager>,
    download_manager: Arc<DownloadStateManager>,
    resource_manager: Arc<ResourceManager>,
    dbus_manager: Option<Arc<DBusManager>>,
}

impl DaemonComponents {
    /// Instantiate all subsystem handles that do not depend on configuration
    /// values (those come from `SuperSTTDaemon::load_and_persist_config`).
    async fn init() -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);
        let audio_processor = Arc::new(AudioProcessor::new());

        // Model slot starts empty; filled once the startup model loads.
        let model: SharedLoadedModel = Arc::new(tokio::sync::RwLock::new(None));

        let realtime_manager = Arc::new(RealTimeTranscriptionManager::new(
            Arc::clone(&model),
            Arc::clone(&audio_processor),
        ));
        let download_manager = Arc::new(DownloadStateManager::new());

        let resource_manager = if cfg!(debug_assertions) {
            Arc::new(ResourceManager::development())
        } else {
            Arc::new(ResourceManager::production())
        };

        // D-Bus is optional; absence is non-fatal.
        let dbus_manager = match DBusManager::new().await {
            Ok(mgr) => Some(Arc::new(mgr)),
            Err(e) => {
                warn!("D-Bus initialization failed (this is normal on some systems): {e}");
                None
            }
        };

        Self {
            shutdown_tx,
            audio_processor,
            model,
            realtime_manager,
            download_manager,
            resource_manager,
            dbus_manager,
        }
    }
}

impl SuperSTTDaemon {
    /// Create a new `SuperSTTDaemon` instance
    ///
    /// # Errors
    ///
    /// Returns an error if model loading fails.
    pub async fn new(
        stt_model_override: Option<String>,
        device_override: Option<DeviceOverride>,
        audio_theme_override: Option<AudioTheme>,
    ) -> Result<Self> {
        info!("Initializing Super STT Daemon...");

        let config = Self::load_and_persist_config(
            stt_model_override,
            device_override,
            audio_theme_override,
        );

        // Extract config fields needed for the struct before config is moved in.
        let preferred_device = config.device.preferred_device.clone();
        let actual_device = preferred_device.clone(); // Will be updated when model loads
        let active_backend = config.transcription.active_backend.clone();
        let preview_typing_enabled = config.transcription.preview_typing_enabled;
        let audio_theme = config.audio.theme;
        let volume = config.audio.volume;

        let components = DaemonComponents::init().await;

        let daemon = SuperSTTDaemon {
            model: components.model,
            audio_processor: components.audio_processor,
            shutdown_tx: components.shutdown_tx,
            dbus_manager: components.dbus_manager,
            realtime_manager: components.realtime_manager,
            events: Arc::new(EventBus::new()),
            audio_theme: Arc::new(RwLock::new(audio_theme)),
            volume: Arc::new(RwLock::new(volume)),
            busy: Arc::new(tokio::sync::RwLock::new(false)),
            download_manager: components.download_manager,
            preferred_device: Arc::new(tokio::sync::RwLock::new(preferred_device)),
            actual_device: Arc::new(tokio::sync::RwLock::new(actual_device)),
            config: Arc::new(tokio::sync::RwLock::new(config)),
            resource_manager: components.resource_manager,
            preview_typing_enabled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
                preview_typing_enabled,
            )),
            manual_stop_tx: Arc::new(tokio::sync::RwLock::new(None)),
            simulator: Arc::new(tokio::sync::RwLock::new(None)),
            preview_text: Arc::new(tokio::sync::RwLock::new(None)),
            backends: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            active_backend: Arc::new(tokio::sync::RwLock::new(active_backend)),
        };

        daemon.post_init(device_override).await;

        Ok(daemon)
    }

    /// Load the daemon config from disk, apply any CLI overrides, and persist
    /// back to disk if anything changed. Returns the ready-to-use config.
    fn load_and_persist_config(
        stt_model_override: Option<String>,
        device_override: Option<DeviceOverride>,
        audio_theme_override: Option<AudioTheme>,
    ) -> DaemonConfig {
        let mut config = DaemonConfig::load();
        info!("Loaded daemon configuration from disk");
        let changed = Self::apply_cli_overrides_to_config(
            &mut config,
            stt_model_override,
            device_override,
            audio_theme_override,
        );
        if changed {
            if let Err(e) = config.save() {
                warn!("Failed to save updated daemon config: {e}");
            } else {
                info!("Updated daemon configuration saved to disk");
            }
        }
        config
    }

    /// Perform post-construction startup: discover backends, apply any
    /// temporary session-level device override, then kick off background
    /// loading of the startup model (if one is configured).
    async fn post_init(&self, device_override: Option<DeviceOverride>) {
        // Discover installed backends.
        self.refresh_backends().await;

        // Apply temporary device override for current session (not saved to config).
        if matches!(device_override, Some(DeviceOverride::Cpu)) {
            let mut preferred_device_guard = self.preferred_device.write().await;
            if *preferred_device_guard != "cpu" {
                info!(
                    "Temporary session override: device preference {} -> cpu (not saved)",
                    *preferred_device_guard
                );
                *preferred_device_guard = "cpu".to_string();
            }
        }

        // Load the configured startup model — if any — in the **background** so
        // the HTTP listener can come up immediately. A model load may download
        // gigabytes and bind a GPU; blocking startup on it would leave clients
        // unable to connect (and unable to pick a lighter model) until it
        // finishes. Clients watch the `daemon_status_changed` SSE topic for the
        // `ready` transition. With no configured preference the daemon stays
        // idle until the user selects a model — it never auto-pulls a model.
        if let Some((name, provider, source)) = self.pick_startup_model().await {
            let bg = self.clone();
            tokio::spawn(async move {
                if let Err(e) = Self::load_initial_model_and_broadcast(
                    &bg,
                    name.clone(),
                    provider.clone(),
                    source,
                )
                .await
                {
                    warn!(
                        "Failed to load startup model {name} via {provider}: {e}; daemon is idle"
                    );
                    bg.download_manager.clear_download();
                }
            });
        } else {
            info!("No startup model configured; daemon is idle until one is selected");
        }
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

    async fn load_initial_model_and_broadcast(
        daemon: &SuperSTTDaemon,
        name: String,
        provider: Provider,
        source: String,
    ) -> Result<()> {
        daemon.broadcast_model_loading_status(&name);

        let device_pref = daemon.preferred_device.read().await.clone();
        let (instance, definition) = daemon
            .instantiate_backend(&name, &provider, &source, &device_pref)
            .await?;

        info!("model {name} via {provider} loaded successfully");
        // Capture the device label before moving `instance` into the shared
        // `LoadedModel` — `ready` event consumers (e.g. the settings app's
        // current_device tracking) only update when `actual_device` is present.
        let actual_device = normalize_device(&instance.device());
        *daemon.actual_device.write().await = actual_device.clone();
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
                "model_name": name,
                "timestamp": chrono::Utc::now().to_rfc3339(),
            }));
        Ok(())
    }

    /// Resolve a wire-level `(name, provider, source)` triple into a
    /// [`ModelDefinition`] from the discovered backends. Returns `None` on miss.
    pub async fn resolve_definition(
        &self,
        name: &str,
        provider: &Provider,
        source: &str,
    ) -> Option<super_stt_shared::models::registry::ModelDefinition> {
        let backends = self.backends.read().await;
        backends::find_model(&backends, name, provider, source).map(|(_, def)| def.clone())
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
    fn normalize_device_maps_labels() {
        assert_eq!(normalize_device("cuda:0"), "cuda");
        assert_eq!(normalize_device("Cuda(0)"), "cuda");
        assert_eq!(normalize_device("Metal(0)"), "metal");
        assert_eq!(normalize_device("remote"), "remote");
        assert_eq!(normalize_device("cpu"), "cpu");
        assert_eq!(normalize_device("anything else"), "cpu");
    }
}
