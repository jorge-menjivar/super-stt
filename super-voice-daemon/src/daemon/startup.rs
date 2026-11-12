// SPDX-License-Identifier: GPL-3.0-only
//! Daemon construction + startup orchestration: assembling subsystem handles,
//! loading + CLI-overriding the config, and kicking off the background load of
//! the configured startup model. Split out of `types.rs` so that file holds
//! just the daemon struct, its type aliases, and runtime accessors.

use crate::config::DaemonConfig;
use crate::daemon::events::EventBus;
use crate::daemon::types::{SharedLoadedModel, SuperSTTDaemon};
use crate::download_progress::DownloadStateManager;
use crate::input::audio::AudioProcessor;
use crate::resource_management::ResourceManager;
use crate::services::dbus::DBusManager;
use crate::stt_models::backends;
use anyhow::Result;
use log::{info, warn};
use std::sync::{Arc, RwLock};
use super_engine_daemon::secrets::Secrets;
use super_voice_shared::SUPER_VOICE;
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
    // `allow`, not `expect`: clippy 1.98 files these under `unused_async_trait_impl`
    // and later releases under `unused_async`, so either `expect` goes unfulfilled
    // on some toolchain.
    #[cfg_attr(
        target_os = "macos",
        allow(
            clippy::unused_async,
            clippy::unused_async_trait_impl,
            reason = "async for the Linux arm's session-bus connect, which macOS has no counterpart to"
        )
    )]
    async fn init() -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);
        let audio_processor = Arc::new(AudioProcessor::new());

        // Model slot starts empty; filled once the startup model loads.
        let model: SharedLoadedModel = Arc::default();

        let download_manager = Arc::new(DownloadStateManager::new());

        let resource_manager = Arc::new(crate::limits::resource_manager());

        // D-Bus is optional; absence is non-fatal.
        #[cfg(target_os = "linux")]
        let dbus_manager = match DBusManager::new().await {
            Ok(mgr) => Some(Arc::new(mgr)),
            Err(e) => {
                warn!("D-Bus initialization failed (this is normal on some systems): {e}");
                None
            }
        };
        // No session bus on macOS, so there is nothing to fail and nothing to
        // warn about — see `services::dbus`. Clients get the same events from
        // `GET /v1/events`.
        #[cfg(target_os = "macos")]
        let dbus_manager: Option<Arc<DBusManager>> = None;

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
    /// Returns an error if the secrets file exists and cannot be read.
    pub async fn new() -> Result<Self> {
        info!("Initializing the Super Voice daemon...");

        // Before anything else that could fail: a daemon that cannot read the
        // file must not start, or its first write would replace a file it
        // never read.
        let secrets = Secrets::open(&SUPER_VOICE)?;

        let config = Self::load_and_persist_config();

        // Extract config fields needed for the struct before config is moved in.
        let active_backend = config.transcription.active_backend.clone();
        let preview_typing_enabled = config.transcription.preview_typing_enabled;
        let audio_theme = config.audio.theme;
        let volume = config.audio.volume;

        let components = DaemonComponents::init().await;
        // One of each for both features: text to speech is built on the same
        // handles below.
        let events = Arc::new(EventBus::new());
        let config = Arc::new(tokio::sync::RwLock::new(config));
        let tts = crate::tts::daemon::types::SuperTTSDaemon::new(
            crate::tts::daemon::types::Shared {
                config: Arc::clone(&config),
                events: Arc::clone(&events),
                secrets: secrets.clone(),
                resource_manager: Arc::clone(&components.resource_manager),
                shutdown_tx: components.shutdown_tx.clone(),
            },
            &*config.read().await,
        );

        let daemon = SuperSTTDaemon {
            model: components.model,
            post_processor: Arc::default(),
            audio_processor: components.audio_processor,
            shutdown_tx: components.shutdown_tx,
            dbus_manager: components.dbus_manager,
            events,
            audio_theme: Arc::new(RwLock::new(audio_theme)),
            volume: Arc::new(RwLock::new(volume)),
            recording: Arc::default(),
            download_manager: components.download_manager,
            config,
            resource_manager: components.resource_manager,
            preview_typing_enabled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
                preview_typing_enabled,
            )),
            simulator: Arc::new(tokio::sync::RwLock::new(None)),
            preview_text: Arc::new(tokio::sync::RwLock::new(None)),
            backends: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            all_backends: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            active_backend: Arc::new(tokio::sync::RwLock::new(active_backend)),
            notifier: Arc::new(tokio::sync::Mutex::new(
                crate::output::notification::Notifier::desktop(),
            )),
            self_update: Arc::new(crate::self_update::checker()),
            shortcuts: Arc::new(crate::shortcuts::Shortcuts::default()),
            secrets,
            tts,
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

    /// Perform post-construction startup: discover backends, then kick off
    /// the background load of every stage's selected model.
    async fn post_init(&self) {
        self.move_old_data().await;
        // Discover installed backends, for both features: the folder is read
        // once.
        self.refresh_backends().await;

        // Every stage of both features loads its selected model in the
        // **background**, so the HTTP listener comes up at once: a load may
        // download gigabytes and bind a GPU, and blocking startup on it would
        // leave clients unable to connect (or to pick a lighter model) until
        // it finishes. Clients watch `daemon_status_changed` for each stage's
        // `ready`. A stage with nothing selected stays empty — the daemon
        // never pulls a model of its own — and one whose model is unloaded
        // when idle is left to the first request that needs it. Each stage
        // loads apart from the others, so one failing costs no other its
        // model.
        let stages = self.all_stages();
        stages.spawn_idle_unload({
            let mut shutdown = self.shutdown_tx.subscribe();
            async move {
                let _ = shutdown.recv().await;
            }
        });
        stages.load_at_startup().await;

        // The shortcuts the user added through the portal are bound again: the
        // portal kept them only while the last daemon held them. In the
        // background, since the portal may be slow to start.
        {
            let bg = self.clone();
            tokio::spawn(async move { bg.bind_saved_shortcuts().await });
        }

        // Periodic self-update check: first ~60s after start (don't compete
        // with model load), then daily. POST /v1/update/check bypasses the
        // check_enabled gate; this loop honors it.
        {
            let bg = self.clone();
            let mut shutdown = self.shutdown_tx.subscribe();
            tokio::spawn(async move {
                tokio::select! {
                    () = tokio::time::sleep(std::time::Duration::from_mins(1)) => {}
                    _ = shutdown.recv() => return,
                }
                let mut interval = tokio::time::interval(std::time::Duration::from_hours(24));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    // interval fires immediately on the first tick, which is
                    // exactly the "check on startup" behavior we want.
                    tokio::select! {
                        _ = interval.tick() => {}
                        _ = shutdown.recv() => return,
                    }
                    if bg.config.read().await.update.check_enabled {
                        let _ = bg.run_self_update_check_and_notify().await;
                    }
                }
            });
        }
    }

    /// Move the backends Super STT and Super TTS installed, and the voices
    /// cloned with Super TTS, into Super Voice's folders, once: a start that
    /// finds none to move does nothing. Only into the default folders, since a
    /// folder the user chose is theirs to fill. A failure is logged, never
    /// fatal: the daemon still starts, with whatever its own folders hold.
    async fn move_old_data(&self) {
        let mut moves: Vec<(&str, std::path::PathBuf, &str, crate::old_data::Usable)> = Vec::new();
        if self.config.read().await.backends_dir.is_none() {
            let into = backends::default_backends_dir();
            for product in crate::old_data::OLD_PRODUCTS {
                moves.push((
                    product,
                    into.clone(),
                    "backend",
                    crate::old_data::readable_backend,
                ));
            }
        }
        let voices = crate::tts::voices::VoiceLibrary::default_dir();
        if self.tts.voices.dir() == voices {
            moves.push(("super-tts", voices, "voice", |_| Ok(())));
        }
        for (product, into, what, usable) in moves {
            let Some(from) = crate::old_data::old_product_dir(&into, product) else {
                continue;
            };
            if let Err(e) = crate::old_data::move_folders(&from, &into, what, usable) {
                warn!(
                    "Could not move {product}'s {what}s from {} into {}: {e}",
                    from.display(),
                    into.display()
                );
            }
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
    /// shows its "no backend selected" empty state and `GET /models` (scoped to
    /// the active backend) returns nothing.
    ///
    /// Guarded on `is_none()`, so it only ever fills a gap: an explicit
    /// selection through `set_active_backend`/`set_model` always wins.
    ///
    /// Persisting is deliberately left to the caller. Keeping this free of
    /// disk I/O is what lets it be unit-tested without writing over the user's
    /// real config.
    pub(in crate::daemon) async fn adopt_active_backend_for(&self, source: &str) -> bool {
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
}

#[cfg(test)]
#[path = "startup_tests.rs"]
mod tests;
