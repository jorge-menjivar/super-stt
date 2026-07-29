// SPDX-License-Identifier: GPL-3.0-only
//! Daemon construction + startup orchestration: assembling subsystem handles,
//! loading + CLI-overriding the config, and kicking off the background load of
//! the configured startup model. Split out of `types.rs` so that file holds
//! just the daemon struct, its type aliases, and runtime accessors.

use crate::config::DaemonConfig;
use crate::daemon::events::EventBus;
use crate::daemon::types::{LoadedModel, SharedLoadedModel, SuperSTTDaemon, normalize_device};
use crate::download_progress::DownloadStateManager;
use crate::input::audio::AudioProcessor;
use crate::resource_management::ResourceManager;
use crate::services::dbus::DBusManager;
use crate::stt_models::backends;
use anyhow::Result;
use log::{info, warn};
use std::sync::{Arc, RwLock};
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
    pub async fn new() -> Result<Self> {
        info!("Initializing Super STT Daemon...");

        let config = Self::load_and_persist_config();

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

        daemon.post_init().await;

        Ok(daemon)
    }

    /// Load the daemon config from disk, apply any CLI overrides, and persist
    /// back to disk if anything changed. Returns the ready-to-use config.
    fn load_and_persist_config() -> DaemonConfig {
        let config = DaemonConfig::load();
        info!("Loaded daemon configuration from disk");
        config
    }

    /// Perform post-construction startup: discover backends, apply any
    /// temporary session-level device override, then kick off background
    /// loading of the startup model (if one is configured).
    async fn post_init(&self) {
        // Discover installed backends.
        self.refresh_backends().await;

        // Load the configured startup model — if any — in the **background** so
        // the HTTP listener can come up immediately. A model load may download
        // gigabytes and bind a GPU; blocking startup on it would leave clients
        // unable to connect (and unable to pick a lighter model) until it
        // finishes. Clients watch the `daemon_status_changed` SSE topic for the
        // `ready` transition. With no configured preference the daemon stays
        // idle until the user selects a model — it never auto-pulls a model.
        if let Some((name, source)) = self.pick_startup_model().await {
            let bg = self.clone();
            tokio::spawn(async move {
                if let Err(e) =
                    Self::load_initial_model_and_broadcast(&bg, name.clone(), source).await
                {
                    warn!("Failed to load startup model {name}: {e}; daemon is idle");
                    bg.download_manager.clear_download();
                }
            });
        } else {
            info!("No startup model configured; daemon is idle until one is selected");
        }
    }

    /// Record the backend serving `source` as the active one when nothing has
    /// selected one yet, returning whether it changed anything.
    ///
    /// The startup path resolves its model from the legacy
    /// `preferred_model`/`preferred_source` config, which
    /// carries no `active_backend` — a config written before that field
    /// existed loads it as `None`. Without this the daemon transcribes
    /// happily while `GET /active_backend` stays null, so the settings app
    /// shows its "no backend loaded" empty state and `GET /models` (scoped to
    /// the active backend) returns nothing.
    ///
    /// Guarded on `is_none()`, so it only ever fills a gap: an explicit
    /// selection through `set_active_backend`/`set_model` always wins.
    ///
    /// Persisting is deliberately left to the caller. Keeping this free of
    /// disk I/O is what lets it be unit-tested without writing over the user's
    /// real config.
    async fn adopt_active_backend_for(&self, source: &str) -> bool {
        if self.active_backend.read().await.is_some() {
            return false;
        }
        let dir_name = {
            let backends = self.backends.read().await;
            backends
                .iter()
                .find(|b| b.source == source)
                .and_then(backends::dir_name)
        };
        let Some(dir) = dir_name else {
            return false;
        };
        *self.active_backend.write().await = Some(dir.clone());
        self.config.write().await.transcription.active_backend = Some(dir);
        true
    }

    async fn load_initial_model_and_broadcast(
        daemon: &SuperSTTDaemon,
        name: String,
        source: String,
    ) -> Result<()> {
        daemon.broadcast_model_loading_status(&name);

        let device_pref = daemon.preferred_device.read().await.clone();
        let (instance, definition) = daemon
            .instantiate_backend(&name, &source, &device_pref)
            .await?;

        info!("model {name} loaded successfully");

        if daemon.adopt_active_backend_for(&source).await
            && let Err(e) = daemon.persist_config().await
        {
            warn!("Failed to persist active_backend after startup load: {e}");
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
        daemon.broadcast_model_active(&name, &source, &actual_device);
        Ok(())
    }
}

#[cfg(test)]
#[path = "startup_tests.rs"]
mod tests;
