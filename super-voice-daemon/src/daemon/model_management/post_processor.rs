// SPDX-License-Identifier: GPL-3.0-only
//! The transcript post-processor's slot: stage 2.
//!
//! The post-processor is an ordinary backend model — discovered, instantiated,
//! and driven exactly like a transcription model — that occupies its own slot
//! ([`SuperSTTDaemon::post_processor`]) and is reached over `POST /v1/process`.
//! It is selected independently of the transcription backend, so the two load
//! and unload separately and a backend switch never disturbs it. Its loads
//! and unloads are every stage's ([`super::stage`]).
//!
//! Everything here is best-effort by design: a post-processor that fails to
//! load costs the user the cleanup, never the transcript (see
//! [`SuperSTTDaemon::post_process_final`]).

use log::{info, warn};

use crate::daemon::device_management::PipelineStage;
use crate::daemon::types::SuperSTTDaemon;
use super_engine_daemon::stage::{Load, StageExt};
use super_voice_shared::models::protocol::DaemonResponse;

impl SuperSTTDaemon {
    /// Re-instantiate the loaded post-processor in place so a changed secret or
    /// option takes effect — the stage-2 twin of
    /// [`handle_reload_active_model`](SuperSTTDaemon::handle_reload_active_model).
    /// No-op when stage 2 is idle; rejected during an active recording.
    pub async fn handle_reload_post_processor(&self) -> DaemonResponse {
        if let Some(resp) = self.guard_model_mutation("reload the post-processor") {
            return resp;
        }
        match self.stage(PipelineStage::PostProcessor).reload().await {
            Load::Unchanged => {
                DaemonResponse::success().with_message("No post-processor to reload".to_string())
            }
            Load::Loaded { .. } => {
                DaemonResponse::success().with_message("Post-processor reloaded".to_string())
            }
            Load::Superseded => super::superseded("reload"),
            Load::Cancelled => {
                DaemonResponse::error("Post-processor reload cancelled due to shutdown")
            }
            Load::Failed { error, .. } => {
                warn!("Post-processor reload failed: {error:#}");
                DaemonResponse::error(&format!("Post-processor reload failed: {error:#}"))
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

    /// Drop the loaded post-processor, keeping the stage as it is, and
    /// announce that stage 2 is empty: for a change of backend, which records
    /// its own state. Nothing is announced when nothing was loaded — an event
    /// saying a stage went idle should mean it was running.
    ///
    /// Any load into the slot in flight finishes first: see
    /// [`ModelSlot::unload`](super_engine_daemon::model_slot::ModelSlot::unload).
    pub(in crate::daemon) async fn unload_post_processor_announced(&self) {
        let Some(unloaded) = self.post_processor.unload().await else {
            return;
        };
        if unloaded.model.is_some() {
            info!("Post-processor unloaded");
            self.announce_stage_empty(PipelineStage::PostProcessor);
        }
    }

    /// Whether a post-processor is currently loaded.
    pub(in crate::daemon) async fn post_processor_loaded(&self) -> bool {
        self.post_processor.read().await.is_some()
    }
}
