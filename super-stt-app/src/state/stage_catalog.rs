// SPDX-License-Identifier: GPL-3.0-only

//! What the daemon says each pipeline stage can be filled with:
//! `GET /pipeline/{stage}/backend/list`.
//!
//! The app used to derive this itself, filtering the `/backend/list` catalog on
//! each model's `role` string — twice over, once for which backends a stage may
//! use and once for which of their models. That worked, and it was still a
//! second implementation of a rule the daemon enforces: `POST /pipeline/{stage}`
//! refuses a backend serving nothing the stage can run, and
//! `POST /pipeline/{stage}/model` refuses a model of the wrong role. A local
//! filter that drifted from either would offer the user something the daemon
//! then rejects — an error discoverable only by choosing it.
//!
//! The answer is narrowed on both axes, so what is held here is *the stage's
//! view of the catalog*: only the backends it can use, each carrying only the
//! models it can run. Every picker on the Models page reads it, and none of
//! them filters.
//!
//! Views render synchronously and cannot await, so the answer lands here when
//! it arrives, the way [`crate::state::device_offers::DeviceOffers`] holds its
//! own. A **miss is not an empty answer**: `None` means the question is still in
//! flight, `Some(&[])` means the daemon says there is nothing — which a picker
//! draws as "install one" rather than as a list still loading.

use crate::daemon::backends::BackendInfo;

/// The stage-scoped catalog the daemon has answered with, per stage.
#[derive(Debug, Default)]
pub struct StageCatalog {
    stages: Vec<(u32, Vec<BackendInfo>)>,
}

impl StageCatalog {
    /// The backends that can fill `stage`, each carrying only the models it can
    /// run there — or `None` if the daemon has not answered yet.
    #[must_use]
    pub fn backends(&self, stage: u32) -> Option<&[BackendInfo]> {
        self.stages
            .iter()
            .find(|(s, _)| *s == stage)
            .map(|(_, backends)| backends.as_slice())
    }

    /// One of them by repo id, when the stage can use it at all.
    #[must_use]
    pub fn backend(&self, stage: u32, source: &str) -> Option<&BackendInfo> {
        self.backends(stage)?.iter().find(|b| b.source == source)
    }

    /// The models `source` serves for `stage`, in manifest order.
    ///
    /// Order is the contract: a dropdown selection is an index into this list,
    /// so the view and the handler must read the same one — which they do,
    /// because there is only this one.
    #[must_use]
    pub fn models(&self, stage: u32, source: &str) -> Vec<String> {
        self.backend(stage, source)
            .map(|b| b.models.iter().map(|m| m.name.clone()).collect())
            .unwrap_or_default()
    }

    /// Record what a stage can be filled with.
    pub fn record(&mut self, stage: u32, backends: Vec<BackendInfo>) {
        match self.stages.iter_mut().find(|(s, _)| *s == stage) {
            Some(slot) => slot.1 = backends,
            None => self.stages.push((stage, backends)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StageCatalog;
    use crate::daemon::backends::{BackendInfo, BackendModel};
    use crate::state::device_offers::{PP_STAGE, STT_STAGE};

    fn model(name: &str) -> BackendModel {
        BackendModel {
            name: name.into(),
            provider: String::new(),
            supported_devices: vec!["cpu".into()],
            estimated_vram_bytes: 0,
            multilingual: false,
            supported_languages: Vec::new(),
            primary_language: "en".into(),
            realtime: false,
            role: "transcription".into(),
        }
    }

    fn backend(source: &str, models: Vec<BackendModel>) -> BackendInfo {
        BackendInfo {
            source: source.into(),
            description: String::new(),
            name: source.into(),
            version: "1.0.0".into(),
            kind: "wasm".into(),
            allowed_hosts: Vec::new(),
            installed_accel: Vec::new(),
            models,
            secrets: Vec::new(),
            options: Vec::new(),
        }
    }

    /// The distinction the store exists to keep: not asked yet is not the same
    /// as nothing to offer. A picker draws the first as loading and the second
    /// as "install one", and collapsing them shows the wrong one on every cold
    /// start.
    #[test]
    fn a_miss_is_not_an_empty_answer() {
        let mut catalog = StageCatalog::default();
        assert!(catalog.backends(STT_STAGE).is_none());

        catalog.record(STT_STAGE, Vec::new());
        assert_eq!(
            catalog.backends(STT_STAGE).map(<[BackendInfo]>::len),
            Some(0)
        );
    }

    /// The stages answer independently, and each answer is already narrowed to
    /// its own models — a backend serving both roles shows each stage only what
    /// that stage can run.
    #[test]
    fn each_stage_holds_its_own_view_of_a_shared_backend() {
        let mut catalog = StageCatalog::default();
        catalog.record(STT_STAGE, vec![backend("src/both", vec![model("whisper")])]);
        catalog.record(PP_STAGE, vec![backend("src/both", vec![model("tidy")])]);

        assert_eq!(catalog.models(STT_STAGE, "src/both"), vec!["whisper"]);
        assert_eq!(catalog.models(PP_STAGE, "src/both"), vec!["tidy"]);
    }

    /// A backend the stage cannot use has no models for it, rather than the
    /// ones it ships for the other stage.
    #[test]
    fn a_backend_this_stage_cannot_use_offers_nothing() {
        let mut catalog = StageCatalog::default();
        catalog.record(STT_STAGE, vec![backend("src/stt", vec![model("whisper")])]);

        assert!(catalog.backend(STT_STAGE, "src/clean").is_none());
        assert!(catalog.models(STT_STAGE, "src/clean").is_empty());
    }

    /// A re-answer replaces rather than stacks: manifest order is the dropdown
    /// order, and an index into it is what a click sends.
    #[test]
    fn a_new_answer_replaces_the_old_one() {
        let mut catalog = StageCatalog::default();
        catalog.record(
            STT_STAGE,
            vec![backend("src/a", vec![model("zeta"), model("alpha")])],
        );
        catalog.record(
            STT_STAGE,
            vec![backend("src/a", vec![model("zeta"), model("beta")])],
        );

        assert_eq!(
            catalog.backends(STT_STAGE).map(<[BackendInfo]>::len),
            Some(1)
        );
        assert_eq!(catalog.models(STT_STAGE, "src/a"), vec!["zeta", "beta"]);
    }
}
