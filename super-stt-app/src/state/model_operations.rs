// SPDX-License-Identifier: GPL-3.0-only

//! The model operation each pipeline stage has in flight.
//!
//! The stages provision independently. The daemon will load a transcription
//! model while a post-processor is still downloading, and reports every load,
//! download and failure under its own `stage`.
//!
//! The app used to keep one operation and a number saying whose it was. One
//! stage's work therefore spoke for both: a post-processor download left the
//! transcription card's Load button disabled, and a stage-2 failure cleared
//! the loaded stage-1 model. Each stage gets its own slot here instead, so a
//! card reads, and writes, only its own.
//!
//! A stage this app does not know is ignored rather than rejected. The numbers
//! arrive off the wire, and a daemon that grows a third stage should leave an
//! older app rendering the two it understands.

use std::time::{Duration, Instant};

pub use super::device_offers::{PP_STAGE, STT_STAGE};

/// A stage's model operation: downloading files, loading them, or done.
#[derive(Debug, Clone, Default)]
pub enum ModelOperationState {
    /// Nothing in flight for this stage.
    #[default]
    Ready,
    /// Downloading model files, with the daemon's latest progress.
    Downloading {
        target_model: String,
        progress: super_stt_shared::models::protocol::DownloadProgress,
    },
    /// Loading the model into memory, after any download finished.
    Loading {
        target_model: String,
        status_message: String,
    },
    /// The operation failed, with the daemon's reason.
    Error { message: String },
}

impl ModelOperationState {
    /// Whether the daemon still owes an outcome for this operation. The stall
    /// watchdog and the progress poll both act only on such a stage: `Ready`
    /// needs no poll, and `Error` must keep its banner rather than be cleared
    /// by one.
    #[must_use]
    pub const fn is_pending(&self) -> bool {
        matches!(self, Self::Loading { .. } | Self::Downloading { .. })
    }
}

/// One stage's operation, plus the clock the stall watchdog reads.
#[derive(Debug, Clone, Default)]
struct StageOperation {
    state: ModelOperationState,
    /// When this stage last showed progress — the operation starting, or any
    /// `download_progress` tick for it. `None` outside an operation. Per stage
    /// because a busy stage would otherwise keep a stalled one's clock alive.
    last_progress_at: Option<Instant>,
}

/// What every pipeline stage has in flight, addressed by the daemon's own
/// stage numbers.
#[derive(Debug, Clone, Default)]
pub struct ModelOperations {
    transcription: StageOperation,
    post_processor: StageOperation,
}

impl ModelOperations {
    /// The state the app opens in: stage 1 is loading, because the first thing
    /// the app does is read what the daemon already has. Every other stage is
    /// idle until the daemon says otherwise.
    #[must_use]
    pub fn opening(status_message: String) -> Self {
        let mut ops = Self::default();
        ops.start(
            STT_STAGE,
            ModelOperationState::Loading {
                target_model: String::new(),
                status_message,
            },
        );
        ops
    }

    fn slot(&self, stage: u32) -> Option<&StageOperation> {
        match stage {
            STT_STAGE => Some(&self.transcription),
            PP_STAGE => Some(&self.post_processor),
            _ => None,
        }
    }

    fn slot_mut(&mut self, stage: u32) -> Option<&mut StageOperation> {
        match stage {
            STT_STAGE => Some(&mut self.transcription),
            PP_STAGE => Some(&mut self.post_processor),
            _ => None,
        }
    }

    /// What `stage` has in flight, or `None` for a stage this app does not
    /// track — which renders as nothing rather than as somebody else's work.
    #[must_use]
    pub fn get(&self, stage: u32) -> Option<&ModelOperationState> {
        self.slot(stage).map(|s| &s.state)
    }

    /// Whether `stage` is free to start an operation. An untracked stage has
    /// nothing in flight, so it reads as ready.
    #[must_use]
    pub fn is_ready(&self, stage: u32) -> bool {
        self.slot(stage)
            .is_none_or(|s| matches!(s.state, ModelOperationState::Ready))
    }

    /// The stages still owed an outcome, in pipeline order. What the progress
    /// poll asks about: each stage is polled for its own download, so a
    /// stage-1 answer can never clear a stage-2 card.
    #[must_use]
    pub fn pending_stages(&self) -> Vec<u32> {
        [STT_STAGE, PP_STAGE]
            .into_iter()
            .filter(|&stage| self.get(stage).is_some_and(ModelOperationState::is_pending))
            .collect()
    }

    /// Record `state` for `stage`, leaving the stall clock alone. For the
    /// terminal writes — an outcome arrived, so there is no longer anything to
    /// time.
    pub fn set(&mut self, stage: u32, operation: ModelOperationState) {
        if let Some(slot) = self.slot_mut(stage) {
            slot.state = operation;
        }
    }

    /// Finish `stage`'s operation, whatever it was.
    pub fn set_ready(&mut self, stage: u32) {
        self.set(stage, ModelOperationState::Ready);
    }

    /// Begin (or advance) an operation on `stage`, restarting its stall clock.
    /// For the writes that mean the daemon is working: a switch starting, a
    /// download tick, a load beginning.
    pub fn start(&mut self, stage: u32, operation: ModelOperationState) {
        if let Some(slot) = self.slot_mut(stage) {
            slot.state = operation;
            slot.last_progress_at = Some(Instant::now());
        }
    }

    /// Put every stage back to idle. For a daemon reconnect, where any
    /// operation the app was tracking belongs to a session that is gone.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Fail every stage whose in-flight operation has shown no progress for
    /// `timeout`, and name them. A stage that has already failed, or has
    /// nothing in flight, is left alone.
    pub fn fail_stalled(&mut self, timeout: Duration, message: &str) -> Vec<u32> {
        let mut stalled = Vec::new();
        for stage in [STT_STAGE, PP_STAGE] {
            let Some(slot) = self.slot_mut(stage) else {
                continue;
            };
            if !slot.state.is_pending() {
                continue;
            }
            if slot
                .last_progress_at
                .is_some_and(|at| at.elapsed() > timeout)
            {
                slot.state = ModelOperationState::Error {
                    message: message.to_string(),
                };
                slot.last_progress_at = None;
                stalled.push(stage);
            }
        }
        stalled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loading(model: &str) -> ModelOperationState {
        ModelOperationState::Loading {
            target_model: model.to_string(),
            status_message: "Loading model...".to_string(),
        }
    }

    #[test]
    fn a_stage_starts_idle_and_reads_back_what_it_was_given() {
        let mut ops = ModelOperations::default();
        assert!(ops.is_ready(STT_STAGE));
        assert!(ops.is_ready(PP_STAGE));

        ops.start(PP_STAGE, loading("s1-mini"));
        assert!(!ops.is_ready(PP_STAGE));
        assert!(matches!(
            ops.get(PP_STAGE),
            Some(ModelOperationState::Loading { target_model, .. }) if target_model == "s1-mini"
        ));
    }

    /// The reported bug: a post-processor download left the transcription
    /// card's Load button disabled, because one state spoke for both stages.
    #[test]
    fn one_stages_operation_leaves_the_other_free() {
        let mut ops = ModelOperations::default();
        ops.start(PP_STAGE, loading("s1-mini"));
        assert!(
            ops.is_ready(STT_STAGE),
            "stage 2's load must not block stage 1"
        );

        ops.start(STT_STAGE, loading("whisper-large"));
        assert!(!ops.is_ready(STT_STAGE));
        assert!(!ops.is_ready(PP_STAGE), "and stage 2 keeps its own");

        ops.set_ready(PP_STAGE);
        assert!(ops.is_ready(PP_STAGE));
        assert!(!ops.is_ready(STT_STAGE), "finishing one leaves the other");
    }

    #[test]
    fn only_the_stages_still_working_are_polled() {
        let mut ops = ModelOperations::default();
        assert!(ops.pending_stages().is_empty());

        ops.start(PP_STAGE, loading("s1-mini"));
        assert_eq!(ops.pending_stages(), vec![PP_STAGE]);

        ops.start(STT_STAGE, loading("whisper-large"));
        assert_eq!(ops.pending_stages(), vec![STT_STAGE, PP_STAGE]);

        // A failure is an outcome: the stage is no longer owed one.
        ops.set(
            STT_STAGE,
            ModelOperationState::Error {
                message: "no".to_string(),
            },
        );
        assert_eq!(ops.pending_stages(), vec![PP_STAGE]);
    }

    #[test]
    fn a_stage_the_app_does_not_know_is_ignored() {
        let mut ops = ModelOperations::default();
        ops.start(7, loading("from-the-future"));
        assert!(ops.get(7).is_none(), "it is rendered by nothing");
        assert!(ops.is_ready(7), "and blocks nothing");
        assert!(ops.is_ready(STT_STAGE), "least of all another stage");
        assert!(ops.pending_stages().is_empty());
    }

    /// A zero timeout makes every in-flight stage overdue at once, which is
    /// the mechanism under test — not the duration.
    #[test]
    fn a_stalled_stage_fails_alone() {
        let mut ops = ModelOperations::default();
        ops.start(PP_STAGE, loading("s1-mini"));
        ops.set(STT_STAGE, ModelOperationState::Ready);

        let stalled = ops.fail_stalled(Duration::ZERO, "stalled");
        assert_eq!(stalled, vec![PP_STAGE]);
        assert!(matches!(
            ops.get(PP_STAGE),
            Some(ModelOperationState::Error { message }) if message == "stalled"
        ));
        assert!(ops.is_ready(STT_STAGE), "an idle stage is not failed");

        // Already failed: nothing left to time out, so it is not re-reported.
        assert!(ops.fail_stalled(Duration::ZERO, "stalled").is_empty());
    }

    #[test]
    fn a_reconnect_drops_every_stages_operation() {
        let mut ops = ModelOperations::default();
        ops.start(STT_STAGE, loading("whisper-large"));
        ops.start(PP_STAGE, loading("s1-mini"));

        ops.reset();
        assert!(ops.is_ready(STT_STAGE));
        assert!(ops.is_ready(PP_STAGE));
    }

    #[test]
    fn the_app_opens_reading_the_transcription_stage() {
        let ops = ModelOperations::opening("Loading initial model state...".to_string());
        assert!(!ops.is_ready(STT_STAGE));
        assert_eq!(ops.pending_stages(), vec![STT_STAGE]);
        assert!(ops.is_ready(PP_STAGE));
    }
}
