// SPDX-License-Identifier: GPL-3.0-only
//! Daemon construction + startup orchestration: assembling subsystem handles,
//! loading + CLI-overriding the config, and kicking off the background load of
//! the configured startup model. Split out of `types.rs` so that file holds
//! just the daemon struct, its type aliases, and runtime accessors.

use crate::config::DaemonConfig;
use crate::daemon::events::EventBus;
use crate::daemon::types::{
    DeviceOverride, LoadedModel, SharedLoadedModel, SuperSTTDaemon, normalize_device,
};
use crate::download_progress::DownloadStateManager;
use crate::input::audio::AudioProcessor;
use crate::resource_management::ResourceManager;
use crate::services::dbus::DBusManager;
use crate::stt_models::backends;
use anyhow::Result;
use log::{info, warn};
use std::sync::{Arc, RwLock};
use super_stt_shared::models::provider::Provider;
use super_stt_shared::theme::AudioTheme;
use tokio::sync::broadcast;

/// Pre-assembled subsystem handles created during daemon startup.
struct DaemonComponents {
    shutdown_tx: broadcast::Sender<()>,
    audio_processor: Arc<AudioProcessor>,
    model: SharedLoadedModel,
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

        // Derive the active backend from the loaded model's source when the
        // daemon started via the legacy `preferred_model`/`preferred_source`
        // config (which don't set `active_backend`). Without this the app
        // shows "no backend loaded" even though a model is actively running.
        if daemon.active_backend.read().await.is_none() {
            let backends = daemon.backends.read().await;
            let dir_name = backends
                .iter()
                .find(|b| b.source == source)
                .and_then(backends::dir_name);
            drop(backends);
            if let Some(ref dir) = dir_name {
                *daemon.active_backend.write().await = Some(dir.clone());
                daemon.config.write().await.transcription.active_backend = Some(dir.clone());
                if let Err(e) = daemon.persist_config().await {
                    warn!("Failed to persist active_backend after startup load: {e}");
                }
            }
        }

        // Capture the device label before moving `instance` into the shared
        // `LoadedModel` — `ready` event consumers (e.g. the settings app's
        // current_device tracking) only update when `actual_device` is present.
        let actual_device = normalize_device(&instance.device());
        *daemon.actual_device.write().await = actual_device.clone();
        *daemon.model.write().await = Some(LoadedModel {
            definition,
            instance,
        });

        // Announce the active model the same way a user-initiated switch does
        // (`model_switched` + `ready`). The startup load formerly emitted only
        // `ready`, so a settings app reconnecting after a daemon restart never
        // learned which model became active and kept showing "no model loaded".
        daemon.broadcast_model_active(&name, &provider, &source, &actual_device);
        Ok(())
    }
}
