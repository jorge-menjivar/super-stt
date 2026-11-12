// SPDX-License-Identifier: GPL-3.0-only
//! Daemon construction + startup orchestration: assembling subsystem handles,
//! loading + CLI-overriding the config, and kicking off the background load of
//! the configured startup model. Split out of `types.rs` so that file holds
//! just the daemon struct, its type aliases, and runtime accessors.

use crate::config::DaemonConfig;
use crate::tts::daemon::types::{Shared, SuperTTSDaemon};
use crate::tts::download_progress::DownloadStateManager;
use crate::tts::tts_models::backends;
use std::sync::Arc;

impl SuperTTSDaemon {
    /// Text to speech's part of the daemon, built on the handles it shares
    /// with speech to text. Starts nothing: the daemon's startup loads every
    /// stage of both features once it is assembled.
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
            model: Arc::default(),
            shutdown_tx: shared.shutdown_tx,
            events: Arc::clone(&shared.events),
            download_manager: Arc::new(DownloadStateManager::new()),
            // Empty until a model loads.
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
            secrets: shared.secrets,
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
    pub(in crate::tts::daemon) async fn adopt_active_backend_for(&self, source: &str) -> bool {
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
}

#[cfg(test)]
#[path = "startup_tests.rs"]
mod tests;
