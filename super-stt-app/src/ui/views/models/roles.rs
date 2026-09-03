// SPDX-License-Identifier: GPL-3.0-only
//! Which backends and models belong to which pipeline stage.
//!
//! A backend may serve transcription models, post-processors, or both, and a
//! stage can only run the models matching its role. Every picker on the Models
//! page filters through here, so a stage never offers something the daemon will
//! refuse — and the two stages cannot drift apart in how they decide.

use crate::daemon::backends::BackendInfo;

/// The `role` a post-processor model declares in its manifest.
pub(crate) const POST_PROCESSOR: &str = "post_processor";

/// Whether a role string names a post-processor. Anything else — including a
/// role a newer backend invented — reads as transcription, matching the
/// manifest's default.
fn is_post_processor(role: &str) -> bool {
    role == POST_PROCESSOR
}

/// The installed backends serving at least one model this stage can run, in
/// catalog order.
pub(crate) fn backends_for(backends: &[BackendInfo], post_processor: bool) -> Vec<&BackendInfo> {
    backends
        .iter()
        .filter(|b| {
            b.models
                .iter()
                .any(|m| is_post_processor(&m.role) == post_processor)
        })
        .collect()
}

/// The models one backend serves for this stage, in manifest order.
///
/// Order matters: a dropdown selection is an index into this list, so the view
/// and the handler must build it the same way — hence one function, called by
/// both.
pub(crate) fn models_for(backend: &BackendInfo, post_processor: bool) -> Vec<String> {
    backend
        .models
        .iter()
        .filter(|m| is_post_processor(&m.role) == post_processor)
        .map(|m| m.name.clone())
        .collect()
}

/// Which stage a model operation belongs to, given only the model's name.
///
/// The daemon's `download_progress` events name a model and nothing else, so
/// this is what decides whose card the progress, the loading line, and the
/// failure that ends it belong on. The transcription stage is asked first: a
/// name both stages' backends serve is the one the transcription card just
/// started a load for. A name nothing installed serves is stage 1's too —
/// including a load this app did not start.
pub(crate) fn stage_for_model(
    backends: &[BackendInfo],
    active_backend: Option<&str>,
    post_processor_source: Option<&str>,
    model: &str,
) -> u32 {
    let serves = |source: Option<&str>, post_processor: bool| {
        source
            .and_then(|source| backends.iter().find(|b| b.source == source))
            .is_some_and(|b| {
                b.models
                    .iter()
                    .any(|m| m.name == model && is_post_processor(&m.role) == post_processor)
            })
    };
    if serves(active_backend, false) {
        return crate::state::device_offers::STT_STAGE;
    }
    if serves(post_processor_source, true) {
        return crate::state::device_offers::PP_STAGE;
    }
    crate::state::device_offers::STT_STAGE
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::backends::BackendModel;
    use crate::state::device_offers::{PP_STAGE, STT_STAGE};

    fn model(name: &str, role: &str) -> BackendModel {
        BackendModel {
            name: name.into(),
            provider: String::new(),
            supported_devices: vec!["cpu".into()],
            estimated_vram_bytes: 0,
            multilingual: false,
            supported_languages: Vec::new(),
            primary_language: "en".into(),
            realtime: false,
            role: role.into(),
        }
    }

    fn backend(source: &str, name: &str, models: Vec<BackendModel>) -> BackendInfo {
        BackendInfo {
            source: source.into(),
            description: String::new(),
            name: name.into(),
            version: "1.0.0".into(),
            kind: "wasm".into(),
            allowed_hosts: Vec::new(),
            installed_accel: Vec::new(),
            models,
            secrets: Vec::new(),
            options: Vec::new(),
        }
    }

    fn catalog() -> Vec<BackendInfo> {
        vec![
            backend(
                "github.com/x/stt-only",
                "STT Only",
                vec![model("whisper", "transcription")],
            ),
            backend(
                "github.com/x/clean-only",
                "Clean Only",
                vec![model("textclean", "post_processor")],
            ),
            backend(
                "github.com/x/combo",
                "Combo",
                vec![
                    model("whisper", "transcription"),
                    model("tidy", "post_processor"),
                ],
            ),
        ]
    }

    /// A post-processor-only backend must never be offered as a transcription
    /// backend: selecting it leaves the model picker empty, and the daemon
    /// refuses the selection anyway.
    #[test]
    fn a_post_processor_only_backend_is_not_a_transcription_backend() {
        let catalog = catalog();
        let names: Vec<&str> = backends_for(&catalog, false)
            .iter()
            .map(|b| b.name.as_str())
            .collect();
        assert_eq!(names, vec!["STT Only", "Combo"]);
    }

    /// And the mirror: a transcription-only backend is not a post-processor.
    #[test]
    fn a_transcription_only_backend_is_not_a_post_processor_backend() {
        let catalog = catalog();
        let names: Vec<&str> = backends_for(&catalog, true)
            .iter()
            .map(|b| b.name.as_str())
            .collect();
        assert_eq!(names, vec!["Clean Only", "Combo"]);
    }

    /// A backend serving both appears in both lists, but each stage sees only
    /// the models it can actually run.
    #[test]
    fn a_dual_role_backend_shows_each_stage_only_its_own_models() {
        let catalog = catalog();
        let combo = &catalog[2];
        assert_eq!(models_for(combo, false), vec!["whisper"]);
        assert_eq!(models_for(combo, true), vec!["tidy"]);
    }

    /// An unknown role reads as transcription, matching the manifest default —
    /// a newer backend's model stays usable rather than vanishing from every
    /// picker.
    #[test]
    fn an_unrecognized_role_reads_as_transcription() {
        let b = backend("github.com/x/new", "New", vec![model("weird", "quantum")]);
        assert_eq!(models_for(&b, false), vec!["weird"]);
        assert!(models_for(&b, true).is_empty());
    }

    /// The reported bug: a post-processor's download reported its progress on
    /// the transcription card, because the only card rendering the operation
    /// was the one that had not started it.
    #[test]
    fn a_post_processors_download_belongs_to_stage_two() {
        let catalog = catalog();
        assert_eq!(
            stage_for_model(
                &catalog,
                Some("github.com/x/stt-only"),
                Some("github.com/x/clean-only"),
                "textclean",
            ),
            PP_STAGE,
        );
    }

    /// And the mirror: the transcription model's own load stays on its card
    /// while a post-processor backend is selected.
    #[test]
    fn a_transcription_models_load_belongs_to_stage_one() {
        let catalog = catalog();
        assert_eq!(
            stage_for_model(
                &catalog,
                Some("github.com/x/stt-only"),
                Some("github.com/x/clean-only"),
                "whisper",
            ),
            STT_STAGE,
        );
    }

    /// A name neither selection serves — a load this app did not start, or a
    /// backend uninstalled mid-download — stays on the transcription card
    /// rather than vanishing from both.
    #[test]
    fn an_unknown_model_belongs_to_stage_one() {
        let catalog = catalog();
        assert_eq!(
            stage_for_model(&catalog, Some("github.com/x/stt-only"), None, "nothing"),
            STT_STAGE,
        );
        assert_eq!(stage_for_model(&[], None, None, "whisper"), STT_STAGE);
    }

    /// A model a backend serves only as a post-processor is stage 2's even
    /// when the same backend fills both stages.
    #[test]
    fn a_dual_role_backend_splits_by_the_models_own_role() {
        let catalog = catalog();
        let combo = Some("github.com/x/combo");
        assert_eq!(stage_for_model(&catalog, combo, combo, "tidy"), PP_STAGE);
        assert_eq!(
            stage_for_model(&catalog, combo, combo, "whisper"),
            STT_STAGE
        );
    }
}
