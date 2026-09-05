// SPDX-License-Identifier: GPL-3.0-only
//! Loading and unloading the transcript post-processor.
//!
//! The post-processor is an ordinary backend model — discovered, instantiated,
//! and driven exactly like a transcription model — that occupies its own slot
//! ([`SuperSTTDaemon::post_processor`]) and is reached over `POST /v1/process`.
//! It is selected independently of the transcription backend, so the two load
//! and unload separately and a backend switch never disturbs it.
//!
//! Everything here is best-effort by design: a post-processor that fails to
//! load costs the user the cleanup, never the transcript (see
//! [`SuperSTTDaemon::post_process_final`]).

use anyhow::{Result, bail};
use log::{info, warn};

use crate::daemon::device_management::PipelineStage;
use crate::daemon::types::{SuperSTTDaemon, normalize_device};
use super_stt_shared::models::protocol::{DaemonResponse, DaemonStatusEvent};

impl SuperSTTDaemon {
    /// Load the post-processor named by the config into its slot, replacing
    /// whatever was there.
    ///
    /// The old instance is released first, so a load that fails leaves the
    /// stage idle — and says so — rather than the previous model running. It
    /// has to be: a subprocess backend cannot be instantiated twice for the
    /// same model, so the replacement cannot be built while the old instance
    /// still holds the name.
    ///
    /// # Errors
    /// Returns an error if no post-processor is selected, the selection does
    /// not resolve to an installed `post_processor`-role model, or
    /// instantiation fails.
    pub(in crate::daemon) async fn load_configured_post_processor(&self) -> Result<()> {
        let (name, source) = {
            let config = self.config.read().await;
            let Some((name, source)) = config.post_processor.selection() else {
                bail!("no post-processing model is selected");
            };
            (name.to_string(), source.to_string())
        };

        let definition = self
            .resolve_definition(&name, &source)
            .await
            .ok_or_else(|| {
                anyhow::anyhow!("no installed backend serves the post-processing model {name}")
            })?;
        // A transcription model in this slot would be driven over a route its
        // backend does not serve, failing on every transcript. Refuse the load
        // instead, so the reason is reported once here rather than as a
        // best-effort warning after each recording.
        if !definition.is_post_processor() {
            bail!("model {name} is a transcription model, not a post-processing model");
        }

        let device_pref = self.config.read().await.effective_device(&source, &name);
        // Stage 2 announces its own load the way stage 1 does — this is the
        // only notice a client that did not start it ever gets, and the daemon
        // loads the post-processor on startup as well as on request.
        self.broadcast_model_loading_status(&name, PipelineStage::PostProcessor);
        // Release the running instance before spawning its replacement, the
        // way every stage-1 load path does. A subprocess backend's identity —
        // the `systemd-run --unit=` name and the socket it listens on — is
        // keyed on (backend, model), so a second instance of the same model
        // cannot coexist with the first: systemd refuses the duplicate unit
        // name outright, and the new spawn's socket cleanup would unlink the
        // live one's socket. Loading first was safe only for in-process (wasm)
        // backends; for a subprocess it failed every in-place reload — a device
        // switch, an option change — with an opaque systemd error while the old
        // instance stayed up. It also keeps one copy of the weights on the GPU
        // at a time.
        //
        // Silent, because the slot is normally refilled in the same breath: the
        // `model_switched`/`ready` pair below is the event for this load,
        // exactly as a stage-1 reload reports itself once. The instance is
        // taken out of the lock before `shutdown()`, which can take seconds.
        self.unload_post_processor().await;
        let (instance, definition) = match self
            .instantiate_backend(&name, &source, &device_pref, PipelineStage::PostProcessor)
            .await
        {
            Ok(loaded) => loaded,
            Err(e) => {
                // The slot was emptied to make room and nothing filled it —
                // announce it, or every client goes on showing a post-processor
                // that is no longer running.
                self.announce_post_processor_idle();
                return Err(e);
            }
        };

        let actual_device = normalize_device(&instance.device());
        *self.post_processor.write().await = Some(crate::daemon::types::LoadedModel {
            definition,
            instance,
        });
        info!("Post-processor loaded: {name} (source={source})");
        self.broadcast_model_active(&name, &source, &actual_device, PipelineStage::PostProcessor);
        Ok(())
    }

    /// Re-instantiate the loaded post-processor in place so a changed secret or
    /// option takes effect — the stage-2 twin of
    /// [`handle_reload_active_model`](SuperSTTDaemon::handle_reload_active_model).
    /// No-op when stage 2 is idle; rejected during an active recording.
    pub async fn handle_reload_post_processor(&self) -> DaemonResponse {
        if let Some(resp) = self.guard_model_mutation("reload the post-processor").await {
            return resp;
        }
        if !self.post_processor_loaded().await {
            return DaemonResponse::success()
                .with_message("No post-processor to reload".to_string());
        }
        match self.load_configured_post_processor().await {
            Ok(()) => DaemonResponse::success().with_message("Post-processor reloaded".to_string()),
            Err(e) => {
                warn!("Post-processor reload failed: {e}");
                DaemonResponse::error(&format!("Post-processor reload failed: {e}"))
            }
        }
    }

    /// The backend serving the loaded post-processor, if one is loaded.
    pub(in crate::daemon) async fn post_processor_source(&self) -> Option<String> {
        self.post_processor
            .read()
            .await
            .as_ref()
            .map(|l| l.definition.source.clone())
    }

    /// Drop the loaded post-processor and announce that stage 2 is idle.
    ///
    /// The unload paths answering a user request use this; the reload path
    /// uses the silent [`unload_post_processor`](Self::unload_post_processor),
    /// whose slot is refilled in the same breath. Nothing is announced when
    /// nothing was loaded — an event saying a stage went idle should mean it
    /// was running.
    pub(in crate::daemon) async fn unload_post_processor_announced(&self) {
        if !self.post_processor_loaded().await {
            return;
        }
        self.unload_post_processor().await;
        self.announce_post_processor_idle();
    }

    /// Announce that stage 2 is running nothing — the event a client needs to
    /// stop showing a post-processor that is gone, whether it was unloaded on
    /// request or a reload emptied the slot and failed to refill it.
    fn announce_post_processor_idle(&self) {
        self.events.publish_daemon_status(DaemonStatusEvent::Ready {
            model_loaded: false,
            model_name: None,
            actual_device: None,
            preferred_device: None,
            stage: PipelineStage::PostProcessor.position(),
        });
    }

    /// Drop the loaded post-processor, if any. Mirrors
    /// [`unload_current_model`](SuperSTTDaemon::unload_current_model): take the
    /// instance out of the lock first, then shut it down with the lock
    /// released.
    pub(in crate::daemon) async fn unload_post_processor(&self) {
        let taken = self.post_processor.write().await.take();
        if let Some(mut loaded) = taken {
            if let Err(e) = loaded.instance.shutdown().await {
                warn!("post-processor shutdown failed: {e}");
            }
            drop(loaded);
            info!("Post-processor unloaded");
        }
    }

    /// Whether a post-processor is currently loaded.
    pub(in crate::daemon) async fn post_processor_loaded(&self) -> bool {
        self.post_processor.read().await.is_some()
    }
}
