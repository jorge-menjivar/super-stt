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
use super_stt_shared::models::protocol::DaemonStatusEvent;

impl SuperSTTDaemon {
    /// Load the post-processor named by the config into its slot, replacing
    /// whatever was there.
    ///
    /// # Errors
    /// Returns an error if no post-processor is selected, the selection does
    /// not resolve to an installed `post_processor`-role model, online models
    /// are disabled and the selection is one, or instantiation fails.
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
        if definition.is_online() && !self.config.read().await.online.allow_online_models {
            bail!(
                "post-processor {name} is an online model and online models are disabled; \
                 enable 'Allow Online Models' in settings first"
            );
        }

        let device_pref = self.config.read().await.effective_device(&source, &name);
        // Stage 2 announces its own load the way stage 1 does — this is the
        // only notice a client that did not start it ever gets, and the daemon
        // loads the post-processor on startup as well as on request.
        self.broadcast_model_loading_status(&name, PipelineStage::PostProcessor);
        let (instance, definition) = self
            .instantiate_backend(&name, &source, &device_pref, PipelineStage::PostProcessor)
            .await?;

        let actual_device = normalize_device(&instance.device());
        // Same take-then-shutdown ordering as the transcription slot: the old
        // instance is released outside the write lock, since a subprocess
        // shutdown can take seconds. Silent, because the slot is about to be
        // refilled: the `model_switched`/`ready` pair below is the event for
        // this load, exactly as a stage-1 reload reports itself once.
        self.unload_post_processor().await;
        *self.post_processor.write().await = Some(crate::daemon::types::LoadedModel {
            definition,
            instance,
        });
        info!("Post-processor loaded: {name} (source={source})");
        self.broadcast_model_active(&name, &source, &actual_device, PipelineStage::PostProcessor);
        Ok(())
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
