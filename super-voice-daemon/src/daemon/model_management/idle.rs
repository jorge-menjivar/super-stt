// SPDX-License-Identifier: GPL-3.0-only
//! Unloading the transcription model when idle and loading it on demand. See
//! [`crate::config::IdleUnloadConfig`]. The post-processor is not covered: it
//! stays loaded while its stage is on.

use crate::daemon::device_management::PipelineStage;
use crate::daemon::types::{PendingStart, SuperSTTDaemon};
use crate::idle_unload::CHECK_EVERY;
use log::{info, warn};
use std::time::Duration;
use super_voice_shared::models::protocol::DaemonStatusEvent;

/// How a recording's wait for its model ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ModelWait {
    /// Go on: the model is in memory, or there was none to load, which the
    /// caller's own `model_not_loaded` check reports.
    Ready,
    /// The selected model was loaded for the recording, and the load failed:
    /// why.
    LoadFailed(String),
    /// The recording was cancelled while its model loaded.
    Cancelled,
    /// Another recording is already waiting for the model.
    AnotherWaiting,
}

impl SuperSTTDaemon {
    /// Record a use of the transcription model and, when models are unloaded
    /// on idle, load the selected one if it is not in memory. Every request
    /// that needs the model calls this first.
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

    /// Whether a request finding no model in memory would load one: models
    /// are unloaded when idle, and one is selected. Quiet: it runs on every
    /// recording.
    pub(crate) async fn loads_on_demand(&self) -> bool {
        self.config.read().await.idle_unload.after().is_some()
            && self.selected_model().await.is_ok()
    }

    /// [`ensure_model_loaded`](Self::ensure_model_loaded) for a recording,
    /// which can be cancelled while it waits: by a stop, or its client going
    /// away.
    ///
    /// A recording that waited keeps its reservation when this returns
    /// [`ModelWait::Ready`]. It gives it up as it claims `busy`
    /// ([`start_reserved_recording`](Self::start_reserved_recording)), or with
    /// [`release_pending_recording`](Self::release_pending_recording) if it
    /// ends before that, so there is no moment between the two when a stop
    /// reaches neither.
    ///
    /// The load itself is not cancelled: see
    /// [`ensure_model_loaded`](Self::ensure_model_loaded).
    pub(crate) async fn ensure_model_loaded_for_recording(&self) -> ModelWait {
        // One recording at a time is on its way to the microphone.
        if self.recording_is_pending() {
            return ModelWait::AnotherWaiting;
        }
        // Before the slot is read, as in `ensure_model_loaded`: touched after,
        // an idle unload could take the model in between on its old time.
        self.model.touch();
        if self.model.read().await.is_some() || !self.loads_on_demand().await {
            return ModelWait::Ready;
        }
        let (cancel, cancelled) = tokio::sync::oneshot::channel();
        {
            // Checked and claimed under one lock: a second claim would
            // replace the first's cancel handle.
            let mut pending = self.pending();
            if pending.is_some() {
                return ModelWait::AnotherWaiting;
            }
            *pending = Some(PendingStart(Some(cancel)));
        }
        let load = self.ensure_model_loaded();
        let outcome = tokio::select! {
            // A cancel that was answered wins over a load that finished at
            // the same moment.
            biased;
            Ok(()) = cancelled => ModelWait::Cancelled,
            loaded = load => loaded.map_or_else(ModelWait::LoadFailed, |()| ModelWait::Ready),
        };
        if outcome == ModelWait::Cancelled {
            self.release_pending_recording();
        }
        outcome
    }

    /// Whether a recording is waiting for its model to load.
    pub(crate) fn recording_is_pending(&self) -> bool {
        self.pending().is_some()
    }

    /// Cancel the recording waiting for its model to load. Returns whether
    /// there was one that had not been cancelled already.
    ///
    /// The reservation is marked, not removed: the recording gives it up
    /// itself, so no other can take its place before it has seen the cancel.
    pub(crate) fn cancel_pending_recording(&self) -> bool {
        let Some(cancel) = self.pending().as_mut().and_then(|start| start.0.take()) else {
            return false;
        };
        info!("Cancelled the recording that was waiting for its model to load");
        // Unreceived when the load has just finished: the mark is what the
        // recording then finds as it claims `busy`.
        let _ = cancel.send(());
        true
    }

    /// Give up the reservation of a recording that ends without capturing.
    pub(crate) fn release_pending_recording(&self) {
        *self.pending() = None;
    }

    /// Give up the reservation as the recording claims `busy`, under whose
    /// write lock the caller calls this. Returns whether to start: `false`
    /// when the recording was cancelled after its model had loaded.
    pub(crate) fn start_reserved_recording(&self) -> bool {
        self.pending().take().is_none_or(|start| start.0.is_some())
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, Option<PendingStart>> {
        self.pending_recording
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Load the selected model if it is not in memory, whatever the setting.
    ///
    /// Waits for any load or unload in flight first, and loads only if the
    /// slot is still empty after it: see
    /// [`ModelSlot::begin_load_if_empty`](super_engine_daemon::model_slot::ModelSlot::begin_load_if_empty).
    /// The selection is read on the load's turn, so an unload that switched
    /// the model off meanwhile is seen.
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
                self.download_manager
                    .clear_download(PipelineStage::Transcription.position());
                format!("{name} did not load: {e:#}")
            })
    }

    /// Unload the transcription model if nothing has used it for `after`,
    /// keeping it selected. Returns whether it was unloaded.
    pub(crate) async fn unload_if_idle(&self, after: Duration) -> bool {
        if *self.busy.read().await {
            self.model.touch();
            return false;
        }
        // A transcription holds the slot's write lock and a real-time session
        // its read lock, which the slot counts as a use.
        self.model
            .unload_if_idle(after, |loaded| {
                info!(
                    "Unloaded {} after {}s idle; it loads again on demand",
                    loaded.definition.name,
                    after.as_secs()
                );
                self.events.publish_daemon_status(DaemonStatusEvent::Ready {
                    model_loaded: false,
                    model_name: None,
                    actual_device: None,
                    preferred_device: None,
                    stage: PipelineStage::Transcription.position(),
                });
            })
            .await
    }

    /// Start the task that unloads the transcription model when idle. It reads
    /// the setting each time, so it runs whether or not the setting is on.
    pub(in crate::daemon) fn spawn_idle_unload(&self) {
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
