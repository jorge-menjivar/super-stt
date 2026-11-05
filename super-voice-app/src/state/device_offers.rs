// SPDX-License-Identifier: GPL-3.0-only

//! What the daemon says can run where: the device lists behind
//! `/stt/pipeline/{stage}/device/list` and
//! `/stt/pipeline/{stage}/model/{model}/device/list`.
//!
//! The app used to derive this itself, intersecting a model's declared
//! `supported_devices` with the backend's installed accelerator. The daemon
//! makes that same narrowing against the host it actually runs on, so the
//! answer is read rather than recomputed — and the two can no longer drift.
//!
//! Views render synchronously and cannot await, so answers land here as they
//! arrive and are read back by `(stage, source[, model])`. Carrying the source
//! in the key is what makes a stale answer harmless: after a backend switch a
//! lookup for the new one simply misses until its own answer lands.

/// One answer from the daemon: the devices available for a stage's backend, or
/// for one model of it.
use super::Stage;

struct Offer {
    stage: Stage,
    source: String,
    /// `None` for the backend-wide list — the union over the models that
    /// backend serves in the stage's role.
    model: Option<String>,
    devices: Vec<String>,
}

/// The device lists the daemon has answered for, per pipeline stage.
///
/// A miss means "not asked yet, or the answer hasn't landed" — never "no
/// devices". Callers must keep the two apart: an empty list is the daemon
/// saying this install can run the model on nothing, which is a blocking
/// state the UI reports; a miss is a request still in flight.
#[derive(Default)]
pub struct DeviceOffers {
    offers: Vec<Offer>,
}

impl DeviceOffers {
    /// The devices `source` can run a stage's models on, or `None` if the
    /// daemon hasn't answered for it.
    pub fn backend(&self, stage: Stage, source: &str) -> Option<&[String]> {
        self.find(stage, source, None)
    }

    /// The devices `model` can be loaded onto in this stage, or `None` if the
    /// daemon hasn't answered for it.
    pub fn model(&self, stage: Stage, source: &str, model: &str) -> Option<&[String]> {
        self.find(stage, source, Some(model))
    }

    /// Record an answer, replacing any it supersedes.
    ///
    /// Answers for the stage's *other* backends are dropped at the same time:
    /// a stage has one selected backend, so the moment one answers for a
    /// source the rest are about a selection that is no longer current.
    pub fn record(
        &mut self,
        stage: Stage,
        source: String,
        model: Option<String>,
        devices: Vec<String>,
    ) {
        self.offers
            .retain(|o| o.stage != stage || o.source == source);
        if let Some(existing) = self
            .offers
            .iter_mut()
            .find(|o| o.stage == stage && o.source == source && o.model == model)
        {
            existing.devices = devices;
            return;
        }
        self.offers.push(Offer {
            stage,
            source,
            model,
            devices,
        });
    }

    fn find(&self, stage: Stage, source: &str, model: Option<&str>) -> Option<&[String]> {
        self.offers
            .iter()
            .find(|o| o.stage == stage && o.source == source && o.model.as_deref() == model)
            .map(|o| o.devices.as_slice())
    }
}

/// The framework a freshly staged model starts on: the one the daemon says it
/// loads on when that is offered, otherwise the first one offered.
///
/// `None` (nothing offered) is the answer that matters: a model that can run
/// on no framework here must not be staged on one, or Load would stay enabled
/// beside the advisory saying it cannot run. Falling back to the first offered
/// framework likewise keeps the staged one inside the set the dropdown
/// renders, so the picker never shows an unselected value.
#[must_use]
pub fn staged_device(devices: &[String], current: Option<String>) -> Option<String> {
    current
        .filter(|d| devices.contains(d))
        .or_else(|| devices.first().cloned())
}

/// Whether the build for `framework` is the one installed, by the daemon's
/// rule: an install with no record (a local import, or a WASM component) is
/// taken at its word, and any other runs the frameworks its record names.
///
/// The daemon offers every framework a published build would run on here,
/// installed or not, and refuses to switch to one whose build is not on disk.
/// This is how the app knows to offer the download first.
#[must_use]
pub fn build_installed(framework: &str, installed_accel: &[String]) -> bool {
    installed_accel.is_empty() || installed_accel.iter().any(|a| a == framework)
}

#[cfg(test)]
mod tests {
    use super::{DeviceOffers, Stage, build_installed, staged_device};

    /// A stage keeps only the selected backend's answers: switching backends
    /// must not leave the previous one's devices readable under the new
    /// selection's stage.
    #[test]
    fn a_new_backends_answer_drops_the_previous_ones() {
        let mut offers = DeviceOffers::default();
        offers.record(Stage::Transcription, "a".into(), None, vec!["cuda".into()]);
        offers.record(
            Stage::Transcription,
            "a".into(),
            Some("m".into()),
            vec!["cuda".into()],
        );
        offers.record(Stage::Transcription, "b".into(), None, vec!["cpu".into()]);

        assert_eq!(
            offers.backend(Stage::Transcription, "b"),
            Some(["cpu".to_string()].as_slice())
        );
        assert_eq!(offers.backend(Stage::Transcription, "a"), None);
        assert_eq!(offers.model(Stage::Transcription, "a", "m"), None);
    }

    /// The two stages answer independently — the same backend serving both
    /// keeps a separate list per stage, because each is scoped to that
    /// stage's role.
    #[test]
    fn the_stages_do_not_overwrite_each_other() {
        let mut offers = DeviceOffers::default();
        offers.record(Stage::Transcription, "a".into(), None, vec!["cuda".into()]);
        offers.record(Stage::PostProcessing, "a".into(), None, vec!["cpu".into()]);

        assert_eq!(
            offers.backend(Stage::Transcription, "a"),
            Some(["cuda".to_string()].as_slice())
        );
        assert_eq!(
            offers.backend(Stage::PostProcessing, "a"),
            Some(["cpu".to_string()].as_slice())
        );
    }

    /// An empty answer is an answer — "this install can run it on nothing" —
    /// and must not read back as the miss that means "still asking".
    #[test]
    fn an_empty_list_is_not_a_miss() {
        let mut offers = DeviceOffers::default();
        offers.record(
            Stage::Transcription,
            "a".into(),
            Some("m".into()),
            Vec::new(),
        );

        assert_eq!(
            offers.model(Stage::Transcription, "a", "m"),
            Some([].as_slice())
        );
        assert_eq!(offers.model(Stage::Transcription, "a", "other"), None);
    }

    /// A second answer for the same key replaces the first rather than
    /// stacking behind it.
    #[test]
    fn a_repeat_answer_replaces_the_earlier_one() {
        let mut offers = DeviceOffers::default();
        offers.record(
            Stage::Transcription,
            "a".into(),
            Some("m".into()),
            vec!["cpu".into()],
        );
        offers.record(
            Stage::Transcription,
            "a".into(),
            Some("m".into()),
            vec!["cpu".into(), "cuda".into()],
        );

        assert_eq!(
            offers.model(Stage::Transcription, "a", "m"),
            Some(["cpu".to_string(), "cuda".to_string()].as_slice())
        );
    }

    /// A model the user once put on CUDA is staged there again.
    #[test]
    fn a_recorded_device_the_daemon_offers_is_restaged() {
        let devices = vec!["cpu".to_string(), "cuda".to_string()];
        assert_eq!(
            staged_device(&devices, Some("cuda".to_string())),
            Some("cuda".to_string())
        );
    }

    /// A recorded framework this machine no longer offers must not be staged
    /// back: the dropdown would render with nothing selected while Load sent
    /// a framework the daemon refuses. The first offered one stands in.
    #[test]
    fn a_recorded_device_the_daemon_no_longer_offers_falls_back() {
        let devices = vec!["cpu".to_string()];
        assert_eq!(
            staged_device(&devices, Some("cuda".to_string())),
            Some("cpu".to_string())
        );
    }

    /// Never loaded, so nothing recorded: the first offered device.
    #[test]
    fn no_recorded_device_takes_the_first_offered() {
        let devices = vec!["cuda".to_string(), "cpu".to_string()];
        assert_eq!(staged_device(&devices, None), Some("cuda".to_string()));
    }

    /// Nothing offered stages nothing — an online model, or a local model
    /// this install can run on no device at all. This `None` is what keeps
    /// the Load button disabled beside the advisory that says why.
    #[test]
    fn an_empty_offer_stages_nothing() {
        assert_eq!(staged_device(&[], Some("cuda".to_string())), None);
        assert_eq!(staged_device(&[], None), None);
    }

    /// No record believes the backend; a record runs only what it names.
    #[test]
    fn a_build_is_installed_when_the_record_names_it() {
        let vulkan = vec!["vulkan".to_string()];
        assert!(build_installed("vulkan", &vulkan));
        assert!(!build_installed("cuda", &vulkan));
        assert!(!build_installed("cpu", &vulkan));
        assert!(build_installed("cuda", &[]));
        let both = vec!["cuda".to_string(), "cpu".to_string()];
        assert!(build_installed("cpu", &both));
    }
}
