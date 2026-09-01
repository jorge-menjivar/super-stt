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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::backends::BackendModel;

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
}
