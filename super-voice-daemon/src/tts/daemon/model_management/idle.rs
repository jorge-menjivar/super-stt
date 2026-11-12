// SPDX-License-Identifier: GPL-3.0-only
//! Unloading the synthesis model when idle and loading it on demand. See
//! [`crate::config::IdleUnloadConfig`].

use crate::idle_unload::CHECK_EVERY;
use crate::tts::daemon::types::SuperTTSDaemon;
use log::{info, warn};
use std::time::Duration;
use super_voice_shared::tts::models::protocol::DaemonStatusEvent;

impl SuperTTSDaemon {
    /// Record a use of the synthesis model and, when models are unloaded on
    /// idle, load the selected one if it is not in memory. Every request that
    /// speaks calls this first.
    ///
    /// Returns once the model is loaded or cannot be: a failure is logged and
    /// left for the caller's own `model_not_loaded` check to report.
    ///
    /// The load runs as its own task, so a request dropped while it waits (its
    /// client gave up or went away) does not cancel it with the backend half
    /// started. The model is then in memory for the next request.
    pub(crate) async fn ensure_model_loaded(&self) {
        // Before the slot is read: the idle check reads this under the slot's
        // write lock, so a request that got this far is never unloaded under.
        self.idle.touch();
        if self.model.read().await.is_some()
            || self.config.read().await.idle_unload.after().is_none()
        {
            return;
        }
        let bg = self.clone();
        let _ = tokio::spawn(async move { bg.load_selected_model().await }).await;
        self.idle.touch();
    }

    /// The model the next request loads when it finds none in memory: the
    /// selected one, when models are unloaded on idle.
    pub(crate) async fn model_loaded_on_demand(
        &self,
    ) -> Option<crate::tts::tts_models::ModelDefinition> {
        self.config.read().await.idle_unload.after()?;
        let (name, source) = self.pick_startup_model().await?;
        self.resolve_definition(&name, &source).await
    }

    /// Load the selected model if it is not in memory, whatever the setting.
    pub(crate) async fn load_selected_model(&self) {
        let _loading = self.idle.loading.lock().await;
        if self.model.read().await.is_some() {
            return;
        }
        let Some((name, source)) = self.pick_startup_model().await else {
            return;
        };
        info!("Loading {name} on demand");
        if let Err(e) = Self::load_initial_model_and_broadcast(self, name.clone(), source).await {
            warn!("Failed to load {name} on demand: {e}");
            self.download_manager.clear_download(());
        }
    }

    /// Unload the synthesis model if nothing has used it for `after`, keeping
    /// it selected. Returns whether it was unloaded.
    pub(crate) async fn unload_if_idle(&self, after: Duration) -> bool {
        if self.speech.current().is_some() {
            self.idle.touch();
            return false;
        }
        let _unloading = self.idle.unloading.lock().await;
        let taken = {
            // Synthesis holds the read lock.
            let Ok(mut slot) = self.model.try_write() else {
                self.idle.touch();
                return false;
            };
            if self.idle.idle_for() < after {
                return false;
            }
            slot.take()
        };
        let Some(mut loaded) = taken else {
            return false;
        };
        let name = loaded.definition.name.clone();
        if let Err(e) = loaded.instance.shutdown().await {
            warn!("backend shutdown failed: {e}");
        }
        drop(loaded);
        info!(
            "Unloaded {name} after {}s idle; it loads again on demand",
            after.as_secs()
        );
        self.events
            .publish_synthesis_status(DaemonStatusEvent::Ready {
                model_loaded: false,
                model_name: None,
                actual_device: None,
                preferred_device: None,
            });
        true
    }

    /// Start the task that unloads the synthesis model when idle. It reads the
    /// setting each time, so it runs whether or not the setting is on.
    pub(in crate::tts::daemon) fn spawn_idle_unload(&self) {
        let bg = self.clone();
        let mut shutdown = self.shutdown_tx.subscribe();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(CHECK_EVERY);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = interval.tick() => {}
                    _ = shutdown.recv() => return,
                }
                let after = bg.config.read().await.idle_unload.after();
                if let Some(after) = after {
                    bg.unload_if_idle(after).await;
                }
            }
        });
    }
}
