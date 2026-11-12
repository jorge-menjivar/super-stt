// SPDX-License-Identifier: GPL-3.0-only
//! Daemon construction + startup orchestration: assembling subsystem handles,
//! loading + CLI-overriding the config, and kicking off the background load of
//! the configured startup model. Split out of `types.rs` so that file holds
//! just the daemon struct, its type aliases, and runtime accessors.

use crate::config::DaemonConfig;
use crate::tts::daemon::types::{LoadedModel, Shared, SuperTTSDaemon, normalize_device};
use crate::tts::download_progress::DownloadStateManager;
use crate::tts::tts_models::backends;
use anyhow::Result;
use log::{info, warn};
use std::sync::Arc;

impl SuperTTSDaemon {
    /// Text to speech's part of the daemon, built on the handles it shares
    /// with speech to text. Starts nothing: [`post_init`](Self::post_init)
    /// does, once the daemon is assembled.
    #[must_use]
    pub fn new(shared: Shared, config: &DaemonConfig) -> Self {
        let active_backend = config.synthesis.active_backend.clone();
        let voices = Arc::new(crate::tts::voices::VoiceLibrary::new(
            crate::tts::voices::VoiceLibrary::default_dir(),
        ));
        // Before the HTTP server exists, so no voice can be mid-create.
        voices.sweep_incomplete();

        SuperTTSDaemon {
            // Empty until a model loads.
            model: Arc::new(tokio::sync::RwLock::new(None)),
            shutdown_tx: shared.shutdown_tx,
            events: Arc::clone(&shared.events),
            download_manager: Arc::new(DownloadStateManager::new()),
            // Empty until a model loads.
            actual_device: Arc::new(tokio::sync::RwLock::new(String::new())),
            config: shared.config,
            resource_manager: shared.resource_manager,
            backends: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            active_backend: Arc::new(tokio::sync::RwLock::new(active_backend)),
            notifier: Arc::new(tokio::sync::Mutex::new(
                crate::output::notification::Notifier::desktop(),
            )),
            // The speech engine publishes onto the daemon's one bus.
            speech: Arc::new(
                crate::tts::daemon::speech::SpeechEngine::new()
                    .with_events(Arc::clone(&shared.events))
                    .with_voices(Arc::clone(&voices)),
            ),
            voices,
            loading: Arc::new(super_engine_daemon::load_gate::LoadGate::default()),
            idle: Arc::default(),
            secrets: shared.secrets,
        }
    }

    /// Perform post-construction startup, once the daemon's refresh has read
    /// this feature's backends: apply any temporary session-level device
    /// override, then kick off background loading of the startup model (if
    /// one is configured).
    pub async fn post_init(&self) {
        // Load the configured startup model — if any — in the **background** so
        // the HTTP listener can come up immediately. A model load may download
        // gigabytes and bind a GPU; blocking startup on it would leave clients
        // unable to connect (and unable to pick a lighter model) until it
        // finishes. Clients watch the `daemon_status_changed` SSE topic for the
        // `ready` transition. With no configured preference the daemon stays
        // idle until the user selects a model — it never auto-pulls a model.
        // Nor does it load one here when models are unloaded on idle: the
        // first request that speaks does.
        self.spawn_idle_unload();
        if self.config.read().await.idle_unload.after().is_some() {
            info!("Models are unloaded when idle; the synthesis model loads on first use");
        } else if let Some((name, source)) = self.pick_startup_model().await {
            let bg = self.clone();
            tokio::spawn(async move {
                if let Err(e) =
                    Self::load_initial_model_and_broadcast(&bg, name.clone(), source).await
                {
                    warn!("Failed to load startup model {name}: {e}; daemon is idle");
                    bg.download_manager.clear_download(());
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
    /// existed loads it as `None`. Without this the daemon speaks
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
        self.config.write().await.synthesis.active_backend = Some(dir);
        true
    }

    pub(in crate::tts::daemon) async fn load_initial_model_and_broadcast(
        daemon: &SuperTTSDaemon,
        name: String,
        source: String,
    ) -> Result<()> {
        daemon.broadcast_model_loading_status(&name);

        // The persisted model comes back on its own framework if it has one,
        // else the best its installed build runs.
        let chosen = daemon
            .config
            .read()
            .await
            .model_device(&source, &name)
            .map(ToString::to_string);
        let (instance, definition) = daemon
            .instantiate_backend(&name, &source, chosen.as_deref())
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
        daemon.idle.touch();
        *daemon.model.write().await = Some(LoadedModel::new(definition, instance));

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
