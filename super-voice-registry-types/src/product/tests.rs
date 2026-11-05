// SPDX-License-Identifier: GPL-3.0-only
//! The Super Voice manifest rules, read through the shared parser.

use super::Generation;
use super::{Contract, ModelRole, Product, SUPER_VOICE, Voice, VoiceKind, VoiceManifestError};
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
    contract = "v4"
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

/// The clients that skip the consent dialog are exactly the workspace's apps
/// and CLIs. One left out would show its user a dialog meant for strangers;
/// one named that is not ours would be trusted for its name alone.
#[test]
fn the_first_party_clients_are_the_workspaces_apps_and_clis() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut ours: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().join("Cargo.toml").is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with("-app") || name.ends_with("-cli"))
        .collect();
    ours.sort();
    let mut named: Vec<&str> = SUPER_VOICE.clients.to_vec();
    named.sort_unstable();
    assert_eq!(named, ours);
}

/// Super Voice's manifests name it by the slug its daemon goes by, so a
/// manifest for the daemon is a manifest for the product.
#[test]
fn a_manifest_names_super_voice() {
    assert_eq!(Voice::SLUG, "super-voice");
    assert_eq!(SUPER_VOICE.slug, Voice::SLUG);
}

/// The versions each generation is reported as needing are the releases
/// that shipped them. A daemon that predates a generation shows this to the
/// user as what to update to.
#[test]
fn each_generation_names_the_release_that_shipped_it() {
    assert_eq!(Contract::V1.min_client(), "0.2.0");
    assert_eq!(Contract::V2.min_client(), "0.2.4-beta.1");
    assert_eq!(Contract::V3.min_client(), "0.3.0");
    assert_eq!(Contract::V4.min_client(), "0.3.0");
    assert_eq!(Contract::LATEST, Contract::V4);
}

/// `VALID` with `contract` set to `contract` and `backend` lines in place of
/// its `id` and `product`, plus one model with `model` inside it.
fn at(contract: &str, backend: &str, model: &str) -> String {
    let head = VALID
        .replace("contract = \"v4\"", &format!("contract = \"{contract}\""))
        .replace("    id = \"com.example.y\"\n", "")
        .replace("    product = \"super-voice\"\n", "");
    format!(
        "{head}
        {backend}
        [[models]]
        name = \"m\"
        primary_language = \"en\"
        supported_languages = [\"en\"]
        supported_devices = [\"cpu\"]
        {model}
        "
    )
}

/// v4 is v3 plus a required `[backend].product`. Both v2 and later require
/// `[backend].id`.
#[test]
fn v4_requires_the_product_and_v2_the_id() {
    let id = "id = \"com.example.y\"";
    match Manifest::parse(&at("v4", id, "")) {
        Err(ManifestError::FieldRequiredByContract {
            field, previous, ..
        }) => {
            assert_eq!(field, "[backend].product");
            assert_eq!(previous.as_deref(), Some("v3"));
        }
        other => panic!("v4 without a product must be refused, got {other:?}"),
    }
    for contract in ["v2", "v3", "v4"] {
        match Manifest::parse(&at(contract, "product = \"super-voice\"", "")) {
            Err(ManifestError::FieldRequiredByContract { field, .. }) => {
                assert_eq!(field, "[backend].id");
            }
            other => panic!("{contract} without an id must be refused, got {other:?}"),
        }
    }
    let m = Manifest::parse(&at("v4", &format!("{id}\nproduct = \"super-voice\""), ""))
        .expect("v4 with a product parses");
    assert_eq!(m.backend.contract, Contract::V4);
    assert_eq!(m.backend.product.as_deref(), Some("super-voice"));
}

/// Below v4 the product is optional, because every backend published before
/// the field existed is a speech-to-text backend of this line. One may still
/// name it. These are the manifests Super STT's published backends carry.
#[test]
fn older_generations_may_name_the_product_or_leave_it_out() {
    let id = "id = \"com.example.y\"";
    for contract in ["v1", "v2", "v3"] {
        let without = Manifest::parse(&at(contract, id, ""))
            .unwrap_or_else(|e| panic!("{contract} without a product: {e}"));
        assert_eq!(without.backend.product, None);
        Manifest::parse(&at(
            contract,
            &format!("{id}\nproduct = \"super-voice\""),
            "",
        ))
        .unwrap_or_else(|e| panic!("{contract} with a product: {e}"));
    }
}

/// `prepare_files` is a v4 field: a v3 manifest that declares it is refused,
/// naming the field, and a v4 one parses with it.
#[test]
fn prepare_files_requires_v4() {
    let id = "id = \"com.example.y\"";
    let model = r#"prepare_files = ["m/config.json"]
        files = [{ url = "https://h/config.json", destination = "m/config.json" }]"#;
    match Manifest::parse(&at("v3", id, model)) {
        Err(ManifestError::FieldRequiresContract {
            field,
            since,
            declared,
        }) => {
            assert_eq!(field, "[[models]].prepare_files");
            assert_eq!(since, "v4");
            assert_eq!(declared, "v3");
        }
        other => panic!("prepare_files under v3 must be refused, got {other:?}"),
    }
    let m = Manifest::parse(&with_model(model)).expect("prepare_files under v4 parses");
    assert_eq!(m.models[0].prepare_files, ["m/config.json"]);
}

/// `role` and `force_preview_support` are v2 fields: a v1 manifest that
/// spells either one is refused, naming the field.
#[test]
fn v2_model_fields_are_refused_under_v1() {
    for key in ["role = \"post_processor\"", "force_preview_support = true"] {
        match Manifest::parse(&at("v1", "", key)) {
            Err(ManifestError::FieldRequiresContract {
                since, declared, ..
            }) => {
                assert_eq!(since, "v2");
                assert_eq!(declared, "v1");
            }
            other => panic!("`{key}` under v1 must be refused, got {other:?}"),
        }
    }
}

/// v3 adds no field, and keeps everything v2 does.
#[test]
fn v3_is_v2_with_nothing_removed() {
    let fields = "role = \"post_processor\"\nforce_preview_support = true";
    for contract in ["v2", "v3"] {
        let m = Manifest::parse(&at(contract, "id = \"com.example.y\"", fields))
            .unwrap_or_else(|e| panic!("{contract}: {e}"));
        assert!(m.models[0].product.role.is_post_processor());
        assert!(m.models[0].product.force_preview_support);
    }
}

/// A model that leaves `role` out transcribes, in a manifest and in an index
/// published before the field existed.
#[test]
fn a_missing_role_reads_as_transcription() {
    let m = Manifest::parse(&at("v1", "", "")).expect("parses");
    assert_eq!(m.models[0].product.role, ModelRole::Transcription);

    let model: IndexModel =
        serde_json::from_str(r#"{"name":"m","supported_devices":["cpu"]}"#).expect("parses");
    assert_eq!(model.product.role, "transcription");
}

/// Text to speech starts at v4: a synthesis model, or a synthesis key, under
/// an older contract is refused. A daemon of that generation would list the
/// backend and then fail to install it.
#[test]
fn synthesis_requires_v4() {
    let id = "id = \"com.example.y\"";
    for contract in ["v2", "v3"] {
        let err = refusal(&at(contract, id, "role = \"synthesis\""));
        assert!(
            matches!(&err, VoiceManifestError::SynthesisRequiresV4 { declared, .. }
                if declared.to_string() == contract),
            "{err:?}"
        );
        assert_eq!(
            err.to_string(),
            format!(
                "model `m` has role = synthesis, which requires `contract = \"v4\"`; this \
                 manifest declares `contract = \"{contract}\"`"
            )
        );
        match Manifest::parse(&at(
            contract,
            id,
            "role = \"synthesis\"\nvoice_kinds = [\"preset\"]",
        )) {
            Err(ManifestError::FieldRequiresContract { field, since, .. }) => {
                assert_eq!(field, "[[models]].voice_kinds");
                assert_eq!(since, "v4");
            }
            other => panic!("voice_kinds under {contract} must be refused, got {other:?}"),
        }
    }
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

/// A role this build does not know is refused, naming it.
#[test]
fn an_unknown_role_is_refused() {
    let err = Manifest::parse(&model("dictation", "")).expect_err("an unknown role is refused");
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
