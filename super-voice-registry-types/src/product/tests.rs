// SPDX-License-Identifier: GPL-3.0-only
//! The Super Voice manifest rules, read through the shared parser.

use super::{ModelRole, Product, SUPER_STT, SUPER_TTS, Voice, VoiceKind, VoiceManifestError};
use crate::index::IndexModel;
use crate::manifest::{Manifest, ManifestError};

const VALID: &str = r#"
    [backend]
    source = "github.com/x/y"
    name = "Y"
    version = "1.0.0"
    kind = "wasm"
    entrypoint = "y.wasm"
    id = "com.example.y"
    product = "super-voice"
    contract = "v1"
    description = "Test backend."
    "#;

/// The Super Voice rule `text` breaks. Panics when it parses, or when
/// something other than a Super Voice rule refuses it.
fn refusal(text: &str) -> VoiceManifestError {
    match Manifest::parse(text) {
        Err(ManifestError::Product(e)) => *e
            .downcast::<VoiceManifestError>()
            .expect("the refusal is a Super Voice rule"),
        other => panic!("expected a Super Voice rule to refuse this, got {other:?}"),
    }
}

/// `VALID` plus one synthesis model, so the voice rules have something to
/// attach to. `body` is appended inside the model table.
fn with_model(body: &str) -> String {
    model("synthesis", body)
}

/// `VALID` plus one model of `role`. `body` is appended inside the model
/// table.
fn model(role: &str, body: &str) -> String {
    format!(
        "{VALID}
        [[models]]
        name = \"m\"
        role = \"{role}\"
        primary_language = \"en\"
        supported_languages = [\"en\", \"es\"]
        supported_devices = [\"cpu\"]
        {body}
        "
    )
}

/// The shared parser already refuses a manifest without the key; this is
/// the half it lets through, a key holding nothing to show.
#[test]
fn an_empty_description_is_refused() {
    for blank in ["\"\"", "\"   \""] {
        let text = VALID.replace("\"Test backend.\"", blank);
        assert!(
            matches!(refusal(&text), VoiceManifestError::EmptyDescription),
            "description = {blank} must be refused"
        );
    }
}

#[test]
fn a_model_declaring_no_voice_fields_accepts_presets_only() {
    let m = Manifest::parse(&with_model("")).expect("voice fields are all optional");
    let model = &m.models[0];
    assert_eq!(model.product.voice_kinds, vec![VoiceKind::Preset]);
    assert!(model.product.voices.is_empty());
    assert!(model.product.default_voice.is_none());
    assert!(model.product.max_input_chars.is_none());
    assert!(model.product.output_sample_rate.is_none());
}

#[test]
fn parses_preset_voices_with_a_default() {
    let m = Manifest::parse(&with_model(
        r#"
        default_voice = "bella"
        output_sample_rate = 24000
        max_input_chars = 500
        voices = [
            { id = "bella", label = "Bella", language = "en", tags = ["warm"] },
            { id = "nova" },
        ]
        "#,
    ))
    .expect("a well-formed voice table parses");
    let model = &m.models[0];
    assert_eq!(model.product.voices.len(), 2);
    assert_eq!(model.product.voices[0].display_name(), "Bella");
    // `label` absent falls back to the bare id.
    assert_eq!(model.product.voices[1].display_name(), "nova");
    assert_eq!(model.product.default_voice.as_deref(), Some("bella"));
    assert_eq!(model.product.max_input_chars, Some(500));
    assert_eq!(model.product.output_sample_rate, Some(24000));
}

#[test]
fn rejects_duplicate_voice_ids() {
    let err = refusal(&with_model(
        r#"
        default_voice = "bella"
        voices = [{ id = "bella" }, { id = "bella" }]
        "#,
    ));
    assert!(matches!(err, VoiceManifestError::DuplicateVoiceId { .. }));
}

#[test]
fn rejects_voices_without_a_default() {
    let err = refusal(&with_model(r#"voices = [{ id = "bella" }]"#));
    assert!(matches!(
        err,
        VoiceManifestError::MissingDefaultVoice { .. }
    ));
}

#[test]
fn rejects_a_default_voice_that_names_nothing() {
    let err = refusal(&with_model(
        r#"
        default_voice = "ghost"
        voices = [{ id = "bella" }]
        "#,
    ));
    assert!(matches!(
        err,
        VoiceManifestError::UnknownDefaultVoice { .. }
    ));
}

#[test]
fn rejects_a_voice_language_the_model_does_not_support() {
    let err = refusal(&with_model(
        r#"
        default_voice = "bella"
        voices = [{ id = "bella", language = "ja" }]
        "#,
    ));
    assert!(matches!(
        err,
        VoiceManifestError::VoiceLanguageUnsupported { .. }
    ));
}

#[test]
fn rejects_an_empty_voice_kinds_list() {
    let err = refusal(&with_model("voice_kinds = []"));
    assert!(matches!(err, VoiceManifestError::VoiceKindsEmpty { .. }));
}

#[test]
fn cloning_requires_a_reference_length_bound() {
    let err = refusal(&with_model(r#"voice_kinds = ["cloned"]"#));
    assert!(matches!(
        err,
        VoiceManifestError::CloneMissingRefSeconds { .. }
    ));

    let m = Manifest::parse(&with_model(
        r#"
        voice_kinds = ["preset", "cloned"]
        clone_ref_seconds = 12.0
        "#,
    ))
    .expect("cloned + a bound parses");
    assert_eq!(m.models[0].product.clone_ref_seconds, Some(12.0));
}

#[test]
fn rejects_a_reference_bound_without_cloning() {
    let err = refusal(&with_model("clone_ref_seconds = 12.0"));
    assert!(matches!(
        err,
        VoiceManifestError::CloneRefSecondsRequiresCloned { .. }
    ));
}

#[test]
fn rejects_a_non_positive_reference_bound() {
    for bad in ["0.0", "-3.0", "nan"] {
        let err = refusal(&with_model(&format!(
            "voice_kinds = [\"cloned\"]\nclone_ref_seconds = {bad}"
        )));
        assert!(
            matches!(err, VoiceManifestError::CloneRefSecondsInvalid { .. }),
            "clone_ref_seconds = {bad} must be rejected, got {err:?}"
        );
    }
}

/// The transcript requirement rides on cloning: on its own it would
/// promise a rule the daemon has no cloned voice to apply it to.
#[test]
fn a_transcript_requirement_needs_cloning() {
    let err = refusal(&with_model("clone_needs_transcript = true"));
    assert!(matches!(
        err,
        VoiceManifestError::CloneTranscriptRequiresCloned { .. }
    ));

    let m = Manifest::parse(&with_model(
        r#"
        voice_kinds = ["cloned"]
        clone_ref_seconds = 20.0
        clone_needs_transcript = true
        "#,
    ))
    .expect("cloning may require a transcript");
    assert!(m.models[0].product.clone_needs_transcript);
    assert!(
        !Manifest::parse(&with_model(""))
            .expect("defaults parse")
            .models[0]
            .product
            .clone_needs_transcript,
        "a model that says nothing needs no transcript"
    );
}

#[test]
fn rejects_a_zero_max_input_chars() {
    let err = refusal(&with_model("max_input_chars = 0"));
    assert!(matches!(err, VoiceManifestError::MaxInputCharsZero { .. }));
}

#[test]
fn rejects_an_implausible_output_sample_rate() {
    for bad in [7999, 192_001] {
        let err = refusal(&with_model(&format!("output_sample_rate = {bad}")));
        assert!(matches!(
            err,
            VoiceManifestError::OutputSampleRateOutOfRange { .. }
        ));
    }
    assert!(Manifest::parse(&with_model("output_sample_rate = 48000")).is_ok());
}

/// Super Voice's manifests name it by its own slug. The daemons of its two
/// features keep their own names until they merge, and both read these
/// manifests.
#[test]
fn a_manifest_names_super_voice() {
    assert_eq!(Voice::SLUG, "super-voice");
    assert_ne!(SUPER_STT.slug, SUPER_TTS.slug);
    assert_eq!(SUPER_STT.index_url, SUPER_TTS.index_url, "one registry");
}

/// v1 requires `[backend].product` and `[backend].id`, and there is no older
/// contract to drop to.
#[test]
fn a_manifest_must_name_its_product_and_id() {
    for (line, field) in [
        ("    product = \"super-voice\"\n", "product"),
        ("    id = \"com.example.y\"\n", "id"),
    ] {
        let without = VALID.replace(line, "");
        assert_ne!(without, VALID, "the test removed `{field}`");
        let err = Manifest::parse(&without).expect_err("v1 requires it");
        assert_eq!(
            err.to_string(),
            format!(
                "`contract = \"v1\"` requires `[backend].{field}`, which this manifest does \
                 not declare — add it"
            )
        );
    }
}

/// `prepare_files` is a v1 field.
#[test]
fn prepare_files_parse_under_v1() {
    let m = Manifest::parse(&with_model(
        r#"prepare_files = ["m/config.json"]
        files = [{ url = "https://h/config.json", destination = "m/config.json" }]"#,
    ))
    .expect("prepare_files under v1 parses");
    assert_eq!(m.models[0].prepare_files, ["m/config.json"]);
}

/// A manifest naming any product but Super Voice is refused, the two it
/// replaces included.
#[test]
fn a_manifest_for_another_product_is_refused() {
    for product in ["super-stt", "super-tts", "super-voices"] {
        let other = VALID.replace("\"super-voice\"", &format!("\"{product}\""));
        match Manifest::parse(&other) {
            Err(ManifestError::OtherProduct { declared, expected }) => {
                assert_eq!(declared, product);
                assert_eq!(expected, "super-voice");
            }
            other => panic!("{product} must be refused, got {other:?}"),
        }
    }
}

/// Every model says what it is for. There is no default: a backend can hold
/// models of either feature, so no role is the obvious one.
#[test]
fn a_model_must_declare_its_role() {
    let without = with_model("").replace("        role = \"synthesis\"\n", "");
    assert!(!without.contains("role ="), "the test removed `role`");
    let err = Manifest::parse(&without).expect_err("role is required");
    assert!(err.to_string().contains("role"), "{err}");
    let unknown = model("dictation", "");
    let err = Manifest::parse(&unknown).expect_err("an unknown role is refused");
    assert!(err.to_string().contains("dictation"), "{err}");
}

/// Each role parses, and the index carries it as the manifest spells it.
#[test]
fn every_role_parses_and_reaches_the_index() {
    for (spelled, role) in [
        ("transcription", ModelRole::Transcription),
        ("post_processor", ModelRole::PostProcessor),
        ("synthesis", ModelRole::Synthesis),
    ] {
        let m = Manifest::parse(&model(spelled, "")).expect("parses");
        assert_eq!(m.models[0].product.role, role);
        assert_eq!(Voice::index_model(&m.models[0]).role, spelled);
    }
    let indexed: IndexModel =
        serde_json::from_str(r#"{"name":"m","supported_devices":["cpu"],"role":"synthesis"}"#)
            .expect("parses");
    assert_eq!(indexed.product.role, "synthesis");
}

/// One backend can serve both features.
#[test]
fn one_backend_holds_models_of_both_features() {
    let text = format!(
        "{}
        [[models]]
        name = \"s\"
        role = \"synthesis\"
        primary_language = \"en\"
        supported_languages = [\"en\"]
        supported_devices = [\"cpu\"]
        ",
        model("transcription", "")
    );
    let m = Manifest::parse(&text).expect("both features in one backend");
    let roles: Vec<_> = m.models.iter().map(|m| m.product.role).collect();
    assert_eq!(roles, [ModelRole::Transcription, ModelRole::Synthesis]);
}

/// A synthesis key on a speech-to-text model, or a speech-to-text key on a
/// synthesis model, is refused: it would mean nothing there.
#[test]
fn a_key_of_another_feature_is_refused() {
    for (role, body, key) in [
        (
            "transcription",
            "voices = [{ id = \"a\" }]\ndefault_voice = \"a\"",
            "default_voice",
        ),
        ("post_processor", "max_input_chars = 10", "max_input_chars"),
        ("transcription", "voice_kinds = [\"cloned\"]", "voice_kinds"),
        (
            "synthesis",
            "force_preview_support = true",
            "force_preview_support",
        ),
    ] {
        match refusal(&model(role, body)) {
            VoiceManifestError::FieldNotForRole { key: refused, .. } => {
                assert_eq!(refused, key, "{role}: {body}");
            }
            other => panic!("{role} with {body} must be refused, got {other:?}"),
        }
    }
    let m = Manifest::parse(&model("transcription", "force_preview_support = true"))
        .expect("a speech-to-text key on a speech-to-text model");
    assert!(m.models[0].product.force_preview_support);
}

/// A file that names a host is refused on a speech-to-text model until the
/// daemon picks between variants there; one that does not is the plain shape
/// and still parses.
#[test]
fn a_file_variant_is_refused_for_speech_to_text() {
    let file = |selector: &str| {
        format!("files = [{{ url = \"https://h/w.bin\", destination = \"w.bin\"{selector} }}]")
    };
    assert!(Manifest::parse(&model("transcription", &file(""))).is_ok());
    assert!(matches!(
        refusal(&model("transcription", &file(", accel = \"cuda\""))),
        VoiceManifestError::FileVariantNotSupported { .. }
    ));
}

/// A post-processor is handed a finished transcript, so it cannot also be a
/// realtime model; a manifest declaring both is refused where it is parsed,
/// which is also where the indexer reads it.
#[test]
fn a_realtime_post_processor_is_refused() {
    let tail = "realtime = true";
    let with_websocket = |text: String| format!("{text}\n[capabilities]\nwebsocket = true\n");
    assert!(matches!(
        refusal(&with_websocket(model("post_processor", tail))),
        VoiceManifestError::RealtimePostProcessor { .. }
    ));
    assert!(
        Manifest::parse(&with_websocket(model("transcription", tail))).is_ok(),
        "a realtime transcription model is fine"
    );
}

/// `[capabilities] context` is read beside the shared `websocket`.
#[test]
fn context_is_read_from_capabilities() {
    let m = Manifest::parse(&format!("{VALID}\n[capabilities]\ncontext = true\n")).expect("parses");
    assert!(m.capabilities.product.context);
    assert!(!m.capabilities.websocket);
}

#[test]
fn rejects_the_reserved_streaming_input_capability() {
    let t = format!("{VALID}\n[capabilities]\nstreaming_input = true\n");
    let err = refusal(&t);
    assert!(matches!(err, VoiceManifestError::StreamingInputReserved));
    // The neighbouring capability is unaffected.
    let ok = format!("{VALID}\n[capabilities]\nwebsocket = true\n");
    assert!(Manifest::parse(&ok).is_ok());
}
