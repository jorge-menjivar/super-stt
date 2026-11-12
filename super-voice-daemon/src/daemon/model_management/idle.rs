// SPDX-License-Identifier: GPL-3.0-only
//! Loading the pipeline on demand when models are unloaded on idle. See
//! [`crate::config::IdleUnloadConfig`]. Each stage's load and unload is the
//! engine's ([`StageExt`]), the same for every stage: this is only which
//! stages a request waits for.

use crate::daemon::device_management::PipelineStage;
use crate::daemon::types::SuperSTTDaemon;
use super_engine_daemon::stage::StageExt;

/// How a recording's wait for its models ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ModelWait {
    /// Go on: the models are in memory, or there was none to load, which the
    /// caller's own `model_not_loaded` check reports.
    Ready,
    /// The transcription model was loaded for the recording, and the load
    /// failed: why.
    LoadFailed(String),
    /// The recording was cancelled while its models loaded.
    Cancelled,
}

impl SuperSTTDaemon {
    /// Record a use of every stage a final transcript runs through and, when
    /// models are unloaded on idle, load those not in memory, all at once.
    ///
    /// # Errors
    /// The transcription model failed to load: which, and why. A later stage
    /// is best-effort, as it is on every transcript: one that failed to load
    /// is logged and announced, and the transcript passes through it
    /// unchanged.
    pub(crate) async fn ensure_pipeline_loaded(&self) -> Result<(), String> {
        let loads = PipelineStage::ALL.map(|stage| {
            let stage = self.stage(stage);
            async move { stage.ensure_loaded().await }
        });
        // In stage order: the first transcribes, and without it there is no
        // transcript to pass on.
        futures::future::join_all(loads)
            .await
            .into_iter()
            .next()
            .unwrap_or(Ok(()))
    }

    /// [`ensure_pipeline_loaded`](Self::ensure_pipeline_loaded) for a
    /// recording, which a stop can cancel while it waits: `cancelled` is its
    /// claim's (see
    /// [`RecordingState::reserve`](crate::daemon::recording::RecordingState::reserve)).
    ///
    /// The loads themselves are not cancelled: the models are then in memory
    /// for the next recording.
    pub(crate) async fn wait_for_pipeline(
        &self,
        cancelled: tokio::sync::oneshot::Receiver<()>,
    ) -> ModelWait {
        tokio::select! {
            // A cancel that was answered wins over a load that finished at
            // the same moment.
            biased;
            Ok(()) = cancelled => ModelWait::Cancelled,
            loaded = self.ensure_pipeline_loaded() => {
                loaded.map_or_else(ModelWait::LoadFailed, |()| ModelWait::Ready)
            }
        }
    }
}
