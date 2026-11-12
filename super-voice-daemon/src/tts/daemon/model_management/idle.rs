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
    /// Returns once the model is loaded or cannot be. A load that failed is
    /// the error, with why, so the caller reports that rather than "no model
    /// loaded"; with no model to load, it is left to the caller's own
    /// `model_not_loaded` check.
    ///
    /// The load runs as its own task, so a request dropped while it waits (its
    /// client gave up or went away) does not cancel it with the backend half
    /// started. The model is then in memory for the next request.
    pub(crate) async fn ensure_model_loaded(&self) -> Result<(), String> {
        // Before the slot is read: the idle check reads this under the slot's
        // write lock, so a request that got this far is never unloaded under.
        self.model.touch();
        if self.model.read().await.is_some()
            || self.config.read().await.idle_unload.after().is_none()
        {
            return Ok(());
        }
        let bg = self.clone();
        let loaded = tokio::spawn(async move { bg.load_selected_model().await })
            .await
            .unwrap_or_else(|e| Err(format!("The load stopped: {e}")));
        self.model.touch();
        loaded
    }

    /// The model the next request loads when it finds none in memory: the
    /// selected one, when models are unloaded on idle. Quiet: the voices page
    /// reads it on every listing.
    pub(crate) async fn model_loaded_on_demand(
        &self,
    ) -> Option<crate::tts::tts_models::ModelDefinition> {
        self.config.read().await.idle_unload.after()?;
        let (name, source) = self.selected_model().await.ok()?;
        self.resolve_definition(&name, &source).await
    }

    /// Load the selected model if it is not in memory, whatever the setting.
    ///
    /// Waits for any load or unload in flight first, and loads only if the
    /// slot is still empty after it: see
    /// [`ModelSlot::begin_load_if_empty`](super_engine_daemon::model_slot::ModelSlot::begin_load_if_empty).
    /// The selection is read on the load's turn, so an unload that cleared
    /// it meanwhile is seen.
    ///
    /// # Errors
    /// Why the load failed. Finding nothing to load is not an error.
    pub(crate) async fn load_selected_model(&self) -> Result<(), String> {
        let Some(turn) = self.model.begin_load_if_empty().await else {
            return Ok(());
        };
        let Some((name, source)) = self.pick_startup_model().await else {
            return Ok(());
        };
        info!("Loading {name} on demand");
        Self::load_initial_model_and_broadcast(self, turn, name.clone(), source)
            .await
            .map_err(|e| {
                warn!("Failed to load {name} on demand: {e:#}");
                self.download_manager.clear_download(());
                format!("{name} did not load: {e:#}")
            })
    }

    /// Unload the synthesis model if nothing has used it for `after`, keeping
    /// it selected. Returns whether it was unloaded.
    pub(crate) async fn unload_if_idle(&self, after: Duration) -> bool {
        if self.is_busy() {
            self.model.touch();
            return false;
        }
        // Synthesis holds the slot's read lock, which the slot counts as a use.
        self.model
            .unload_if_idle(after, |loaded| {
                info!(
                    "Unloaded {} after {}s idle; it loads again on demand",
                    loaded.definition.name,
                    after.as_secs()
                );
                self.events
                    .publish_synthesis_status(DaemonStatusEvent::Ready {
                        model_loaded: false,
                        model_name: None,
                        actual_device: None,
                        preferred_device: None,
                    });
            })
            .await
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
