// SPDX-License-Identifier: GPL-3.0-only

//! What each pipeline stage's card has picked but not yet loaded.
//!
//! A pick is local until the Load button commits it: the dropdowns write here,
//! the daemon learns about it only on Load. Every stage's card works that way,
//! so there is one store for all of them, keyed by stage — the same shape as
//! [`crate::state::device_offers::DeviceOffers`] and
//! [`crate::state::model_operations::ModelOperations`].
//!
//! It was two hand-maintained twins before (`staged_model`/`staged_device` for
//! stage 1, `staged_post_processor`/`staged_post_processor_device` for stage
//! 2), and they drifted: stage 2's device answer matched on the model name
//! while ignoring which backend it was staged against, so a same-named model on
//! another backend claimed it.

/// A staged pick: the model, the backend serving it, and the device it would
/// load onto.
///
/// `source` is part of the pick rather than read from elsewhere because a pick
/// outlives the selection that produced it: the answer to "is this pick still
/// this card's?" has to be answerable from the pick itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedPick {
    pub model: String,
    pub source: String,
    /// `None` until the daemon answers what the model can run on here — and
    /// still `None` afterwards when it answers "nothing", which is the case
    /// that must keep Load disabled rather than guess a device.
    pub device: Option<String>,
}

/// One staged pick per stage.
#[derive(Debug, Default)]
pub struct StagedPicks {
    picks: Vec<(u32, StagedPick)>,
}

impl StagedPicks {
    /// The pick staged for `stage`, whichever backend it belongs to.
    #[must_use]
    pub fn get(&self, stage: u32) -> Option<&StagedPick> {
        self.picks
            .iter()
            .find(|(s, _)| *s == stage)
            .map(|(_, pick)| pick)
    }

    /// The pick staged for `stage` *against `source`*.
    ///
    /// A pick staged against another backend is not this card's, and rendering
    /// it would offer a model the card's own dropdown does not list.
    #[must_use]
    pub fn for_backend(&self, stage: u32, source: &str) -> Option<&StagedPick> {
        self.get(stage).filter(|pick| pick.source == source)
    }

    /// The model staged for `stage` against `source`.
    #[must_use]
    pub fn model(&self, stage: u32, source: &str) -> Option<&str> {
        self.for_backend(stage, source)
            .map(|pick| pick.model.as_str())
    }

    /// The device staged for `stage`, when a pick is staged at all.
    #[must_use]
    pub fn device(&self, stage: u32) -> Option<&str> {
        self.get(stage).and_then(|pick| pick.device.as_deref())
    }

    /// Stage `model` from `source`, replacing whatever `stage` had.
    ///
    /// The device is cleared with it: it belonged to the previous model, and
    /// carrying it over would load the new one onto a device nobody chose for
    /// it.
    pub fn stage_model(&mut self, stage: u32, source: String, model: String) {
        let pick = StagedPick {
            model,
            source,
            device: None,
        };
        match self.picks.iter_mut().find(|(s, _)| *s == stage) {
            Some(slot) => slot.1 = pick,
            None => self.picks.push((stage, pick)),
        }
    }

    /// Set the device for `stage`'s pick. Does nothing when nothing is staged —
    /// a device with no model to load is not a pick.
    pub fn set_device(&mut self, stage: u32, device: Option<String>) {
        if let Some((_, pick)) = self.picks.iter_mut().find(|(s, _)| *s == stage) {
            pick.device = device;
        }
    }

    /// Forget `stage`'s pick.
    pub fn clear(&mut self, stage: u32) {
        self.picks.retain(|(s, _)| *s != stage);
    }
}

#[cfg(test)]
mod tests {
    use super::StagedPicks;
    use crate::state::device_offers::{PP_STAGE, STT_STAGE};

    #[test]
    fn a_pick_is_scoped_to_the_backend_it_was_staged_against() {
        let mut picks = StagedPicks::default();
        picks.stage_model(STT_STAGE, "src/a".into(), "whisper".into());

        assert_eq!(picks.model(STT_STAGE, "src/a"), Some("whisper"));
        // The regression: a same-named model on another backend is not this
        // pick, and stage 2 used to match it by name alone.
        assert_eq!(picks.model(STT_STAGE, "src/b"), None);
    }

    /// The stages stage independently — one card's pick must never show on the
    /// other's.
    #[test]
    fn the_stages_do_not_overwrite_each_other() {
        let mut picks = StagedPicks::default();
        picks.stage_model(STT_STAGE, "src/a".into(), "whisper".into());
        picks.stage_model(PP_STAGE, "src/b".into(), "s1-mini".into());

        assert_eq!(picks.model(STT_STAGE, "src/a"), Some("whisper"));
        assert_eq!(picks.model(PP_STAGE, "src/b"), Some("s1-mini"));
    }

    /// Re-staging drops the previous model's device, or the new model would be
    /// loaded onto a device chosen for a different one.
    #[test]
    fn staging_a_new_model_clears_the_device() {
        let mut picks = StagedPicks::default();
        picks.stage_model(STT_STAGE, "src/a".into(), "whisper".into());
        picks.set_device(STT_STAGE, Some("gpu".into()));
        assert_eq!(picks.device(STT_STAGE), Some("gpu"));

        picks.stage_model(STT_STAGE, "src/a".into(), "whisper-large".into());
        assert_eq!(picks.device(STT_STAGE), None);
    }

    /// A device with no model staged is not a pick, so it is dropped rather
    /// than creating one out of nothing.
    #[test]
    fn a_device_without_a_model_stages_nothing() {
        let mut picks = StagedPicks::default();
        picks.set_device(STT_STAGE, Some("gpu".into()));
        assert!(picks.get(STT_STAGE).is_none());
    }

    #[test]
    fn clearing_one_stage_leaves_the_other() {
        let mut picks = StagedPicks::default();
        picks.stage_model(STT_STAGE, "src/a".into(), "whisper".into());
        picks.stage_model(PP_STAGE, "src/b".into(), "s1-mini".into());

        picks.clear(STT_STAGE);
        assert!(picks.get(STT_STAGE).is_none());
        assert_eq!(picks.model(PP_STAGE, "src/b"), Some("s1-mini"));
    }
}
