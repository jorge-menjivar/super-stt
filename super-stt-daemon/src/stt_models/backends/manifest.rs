// SPDX-License-Identifier: GPL-3.0-only
//! Runtime-policy validation of a backend's `backend.toml`. The manifest
//! types and parser are canonical in `super-stt-registry-types`; this module
//! re-exports them and adds the checks only the daemon cares about.

use anyhow::Result;

pub use super_stt_registry_types::manifest::*;

/// Path segments that name an operation rather than a resource, and so cannot
/// also name one.
///
/// Every collection in the API is addressed as `{noun}/list` for the whole set
/// and `{noun}/{name}` for one member. That is a good shape — it never mistakes
/// a member for the collection, and it reads the same at every level — but it
/// puts `list` and a member name in the same path segment, and a static segment
/// wins the route. A backend declaring an option called `list` would therefore
/// have it appear in the listing and be unreachable: no read, no write, no
/// clear, and nothing anywhere saying why.
///
/// Refusing the name at discovery is what makes the shape safe. The alternative
/// — percent-encoding, or a `?name=` query — costs every client something to
/// protect a name nobody wants.
const RESERVED_SEGMENTS: &[&str] = &["list"];

/// Reject an option or secret name that collides with a sibling route.
fn check_addressable_name(kind: &str, name: &str) -> Result<()> {
    if RESERVED_SEGMENTS.contains(&name) {
        anyhow::bail!(
            "{kind} `{name}` cannot be named that: `{name}` addresses the whole \
             collection at /backend/{{backend_id}}/{kind}/{name}, so a {kind} with \
             that name would be listed and then unreachable. Rename it."
        );
    }
    Ok(())
}

/// Validate cross-field invariants the daemon enforces at discovery.
///
/// # Errors
/// Returns an error if a subprocess backend declares the wasm-only
/// `websocket` capability or a non-empty `allowed_hosts` (the transport
/// provides no network), if an option or secret is named after a reserved path
/// segment (see [`RESERVED_SEGMENTS`]), if a model's `primary_language` is
/// absent from its `supported_languages`, if a non-multilingual model's
/// `supported_languages` is not exactly `[primary_language]`, if a model sets
/// `realtime` without the `websocket` capability, or if a post-processor model
/// sets `realtime`.
pub fn validate_runtime(m: &Manifest) -> Result<()> {
    if m.backend.kind == Kind::Subprocess && m.capabilities.websocket {
        anyhow::bail!(
            "[capabilities].websocket is wasm-only; subprocess backends cannot declare it"
        );
    }
    if m.backend.kind == Kind::Subprocess && !m.network.allowed_hosts.is_empty() {
        anyhow::bail!(
            "[network].allowed_hosts must be empty for subprocess backends; the transport provides no network"
        );
    }
    for opt in &m.options {
        check_addressable_name("option", &opt.name)?;
    }
    for secret in &m.secrets {
        check_addressable_name("secret", &secret.name)?;
    }
    for model in &m.models {
        if !model.supported_languages.contains(&model.primary_language) {
            anyhow::bail!(
                "model `{}` primary_language `{}` is not in supported_languages",
                model.name,
                model.primary_language
            );
        }
        if !model.multilingual
            && model.supported_languages.as_slice() != [model.primary_language.clone()]
        {
            anyhow::bail!(
                "model `{}` has multilingual = false but supported_languages is not exactly [primary_language]",
                model.name
            );
        }
        if model.realtime && !m.capabilities.websocket {
            anyhow::bail!(
                "model `{}` has realtime = true but capabilities.websocket is not set",
                model.name
            );
        }
        // Realtime is a property of streaming audio in; a post-processor is
        // handed a finished transcript over `POST /v1/process` and never sees
        // the WebSocket path. Declaring both is a manifest contradiction, so
        // it is refused here rather than silently ignored at load.
        if model.realtime && model.role.is_post_processor() {
            anyhow::bail!(
                "model `{}` has role = post_processor but realtime = true; \
                 post-processors are driven over POST /v1/process, not the realtime path",
                model.name
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A secret with `label` set is parsed; the explicit human-readable text is
    /// what the settings UI shows beside the input.
    #[test]
    fn secret_label_parses_when_present() {
        let toml_src = r#"
[backend]
source = "github.com/super-stt/openai"
name = "OpenAI"
version = "0.1.0"
kind = "wasm"
entrypoint = "openai.wasm"
contract = "v1"
description = "Test backend."

[[secrets]]
name = "openai_api_key"
label = "OpenAI API key"
description = "Authenticate requests."
required = true
"#;
        let manifest = Manifest::parse(toml_src).expect("parse");
        assert_eq!(manifest.secrets.len(), 1);
        assert_eq!(manifest.secrets[0].name, "openai_api_key");
        assert_eq!(manifest.secrets[0].label.as_deref(), Some("OpenAI API key"));
        assert!(manifest.secrets[0].required);
    }

    /// A secret without `label` parses with `label = None`; the UI falls back
    /// to `name` (covered by the `secret_row` view; here we just verify that
    /// the absence is represented faithfully).
    #[test]
    fn secret_label_absent_is_none() {
        let toml_src = r#"
[backend]
source = "github.com/super-stt/openai"
name = "OpenAI"
version = "0.1.0"
kind = "wasm"
entrypoint = "openai.wasm"
contract = "v1"
description = "Test backend."

[[secrets]]
name = "openai_api_key"
description = "Authenticate requests."
"#;
        let manifest = Manifest::parse(toml_src).expect("parse");
        assert_eq!(manifest.secrets.len(), 1);
        assert!(manifest.secrets[0].label.is_none());
        assert!(!manifest.secrets[0].required);
    }

    #[test]
    fn capabilities_websocket_parses_when_true() {
        let toml_src = r#"
[backend]
source = "github.com/super-stt/mistral"
name = "Mistral"
version = "0.2.0"
kind = "wasm"
entrypoint = "mistral.wasm"
contract = "v1"
description = "Test backend."

[capabilities]
websocket = true
"#;
        let m = Manifest::parse(toml_src).expect("parse");
        assert!(m.capabilities.websocket);
    }

    #[test]
    fn capabilities_websocket_defaults_false_when_absent() {
        let toml_src = r#"
[backend]
source = "github.com/super-stt/openai"
name = "OpenAI"
version = "0.1.0"
kind = "wasm"
entrypoint = "openai.wasm"
contract = "v1"
description = "Test backend."
"#;
        let m = Manifest::parse(toml_src).expect("parse");
        assert!(!m.capabilities.websocket);
    }

    #[test]
    fn model_realtime_parses_when_set() {
        let toml_src = r#"
[backend]
source = "github.com/super-stt/mistral"
name = "Mistral"
version = "0.2.0"
kind = "wasm"
entrypoint = "mistral.wasm"
contract = "v1"
description = "Test backend."

[capabilities]
websocket = true

[[models]]
name = "voxtral-mini-transcribe-realtime-2602"
multilingual = true
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["none"]
realtime = true
"#;
        let m = Manifest::parse(toml_src).expect("parse");
        assert!(m.models[0].realtime);
    }

    #[test]
    fn model_realtime_defaults_false() {
        let toml_src = r#"
[backend]
source = "github.com/super-stt/mistral"
name = "Mistral"
version = "0.1.0"
kind = "wasm"
entrypoint = "mistral.wasm"
contract = "v1"
description = "Test backend."

[[models]]
name = "voxtral-mini-latest"
multilingual = true
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["none"]
"#;
        let m = Manifest::parse(toml_src).expect("parse");
        assert!(!m.models[0].realtime);
    }

    #[test]
    fn subprocess_with_websocket_capability_is_rejected() {
        let toml_src = r#"
[backend]
source = "github.com/super-stt/whisper"
name = "Whisper"
version = "0.1.0"
kind = "subprocess"
entrypoint = "super-stt-backend-whisper"
contract = "v1"
description = "Test backend."

[capabilities]
websocket = true
"#;
        let m = Manifest::parse(toml_src).expect("parse");
        let err = validate_runtime(&m).expect_err("subprocess + websocket must fail");
        assert!(err.to_string().contains("wasm-only"), "got: {err}");
    }

    #[test]
    fn subprocess_with_allowed_hosts_is_rejected() {
        let toml_src = r#"
[backend]
source = "github.com/super-stt/whisper"
name = "Whisper"
version = "0.1.0"
kind = "subprocess"
entrypoint = "whisper-backend"
contract = "v1"
description = "Test backend."

[network]
allowed_hosts = ["api.example.com"]
"#;
        let m = Manifest::parse(toml_src).expect("parse");
        let err = validate_runtime(&m).expect_err("subprocess + allowed_hosts must fail");
        assert!(err.to_string().contains("allowed_hosts"), "got: {err}");
    }

    #[test]
    fn wasm_with_allowed_hosts_is_accepted() {
        let toml_src = r#"
[backend]
source = "github.com/super-stt/openai"
name = "OpenAI"
version = "0.1.0"
kind = "wasm"
entrypoint = "openai.wasm"
contract = "v1"
description = "Test backend."

[network]
allowed_hosts = ["api.openai.com"]
"#;
        let m = Manifest::parse(toml_src).expect("parse");
        validate_runtime(&m).expect("wasm + allowed_hosts is permitted");
    }

    #[test]
    fn primary_language_not_in_supported_is_rejected() {
        let toml_src = r#"
[backend]
source = "github.com/super-stt/whisper"
name = "Whisper"
version = "0.1.0"
kind = "subprocess"
entrypoint = "whisper-backend"
contract = "v1"
description = "Test backend."

[[models]]
name = "whisper-tiny"
multilingual = true
primary_language = "en"
supported_languages = ["es", "fr"]
supported_devices = ["cpu"]
"#;
        let m = Manifest::parse(toml_src).expect("parse");
        let err = validate_runtime(&m)
            .expect_err("primary_language outside supported_languages must fail");
        assert!(err.to_string().contains("primary_language"), "got: {err}");
    }

    #[test]
    fn multilingual_false_with_extra_languages_is_rejected() {
        let toml_src = r#"
[backend]
source = "github.com/super-stt/whisper"
name = "Whisper"
version = "0.1.0"
kind = "subprocess"
entrypoint = "whisper-backend"
contract = "v1"
description = "Test backend."

[[models]]
name = "whisper-en"
multilingual = false
primary_language = "en"
supported_languages = ["en", "es"]
supported_devices = ["cpu"]
"#;
        let m = Manifest::parse(toml_src).expect("parse");
        let err =
            validate_runtime(&m).expect_err("multilingual = false with extra languages must fail");
        assert!(err.to_string().contains("multilingual"), "got: {err}");
    }

    #[test]
    fn multilingual_false_with_exact_primary_language_is_accepted() {
        let toml_src = r#"
[backend]
source = "github.com/super-stt/whisper"
name = "Whisper"
version = "0.1.0"
kind = "subprocess"
entrypoint = "whisper-backend"
contract = "v1"
description = "Test backend."

[[models]]
name = "whisper-en"
multilingual = false
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu"]
"#;
        let m = Manifest::parse(toml_src).expect("parse");
        validate_runtime(&m)
            .expect("multilingual = false with exactly [primary_language] is valid");
    }

    #[test]
    fn realtime_model_without_websocket_capability_is_rejected() {
        let toml_src = r#"
[backend]
source = "github.com/super-stt/mistral"
name = "Mistral"
version = "0.2.0"
kind = "wasm"
entrypoint = "mistral.wasm"
contract = "v1"
description = "Test backend."

[[models]]
name = "voxtral-mini-transcribe-realtime-2602"
multilingual = true
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["none"]
realtime = true
"#;
        let m = Manifest::parse(toml_src).expect("parse");
        let err = validate_runtime(&m)
            .expect_err("realtime model without websocket capability must fail");
        assert!(
            err.to_string().contains("capabilities.websocket"),
            "got: {err}"
        );
    }

    /// Options likewise carry an optional `label`; presence and absence both
    /// parse, and the default value/type survive the round-trip.
    #[test]
    fn option_label_and_default_parse() {
        let toml_src = r#"
[backend]
source = "github.com/super-stt/openai"
name = "OpenAI"
version = "0.1.0"
kind = "wasm"
entrypoint = "openai.wasm"
contract = "v1"
description = "Test backend."

[[options]]
name = "region"
label = "Upstream region"
description = "Override the upstream region."
type = "string"
default = "us-east-1"

[[options]]
name = "request_timeout_seconds"
description = "Per-request timeout."
type = "integer"
default = 30
"#;
        let manifest = Manifest::parse(toml_src).expect("parse");
        assert_eq!(manifest.options.len(), 2);

        let region = &manifest.options[0];
        assert_eq!(region.name, "region");
        assert_eq!(region.label.as_deref(), Some("Upstream region"));
        assert_eq!(region.r#type, Some(OptionType::String));
        assert_eq!(
            region.default,
            Some(OptionDefault::String("us-east-1".into()))
        );

        let timeout = &manifest.options[1];
        assert!(timeout.label.is_none(), "label is optional");
        assert_eq!(timeout.r#type, Some(OptionType::Integer));
        assert_eq!(timeout.default, Some(OptionDefault::Integer(30)));
    }

    #[test]
    fn load_rejects_unsafe_entrypoint() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("backend.toml"),
            r#"
[backend]
source = "github.com/x/y"
name = "Y"
version = "1.0.0"
kind = "subprocess"
entrypoint = "/usr/bin/python3"
contract = "v1"
description = "Test backend."
"#,
        )
        .unwrap();
        let err = Manifest::load(dir.path()).expect_err("absolute entrypoint must be rejected");
        // Manifest::load wraps parse errors in Parse{..} — assert the outer
        // variant is Parse containing an inner UnsafeEntrypoint.
        assert!(
            matches!(&err, ManifestError::Parse { err, .. } if matches!(**err, ManifestError::UnsafeEntrypoint(_))),
            "expected Parse {{ UnsafeEntrypoint }}, got: {err}"
        );
    }
    /// `role` defaults to transcription, so every manifest written before the
    /// field existed keeps serving the models it always did.
    #[test]
    fn a_model_without_a_role_is_a_transcription_model() {
        let toml_src = r#"
[backend]
source = "github.com/x/y"
name = "Y"
version = "0.1.0"
kind = "wasm"
entrypoint = "y.wasm"
contract = "v1"
description = "Test backend."

[[models]]
name = "whisper-1"
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["none"]
"#;
        let m = Manifest::parse(toml_src).expect("parse");
        assert_eq!(m.models[0].role, ModelRole::Transcription);
        assert!(!m.models[0].role.is_post_processor());
    }

    #[test]
    fn a_post_processor_role_parses() {
        let toml_src = r#"
[backend]
source = "github.com/x/y"
name = "Y"
version = "0.1.0"
kind = "wasm"
entrypoint = "y.wasm"
contract = "v2"
id = "app.super-stt.openai"
description = "Test backend."

[[models]]
name = "cleanup"
role = "post_processor"
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["none"]
"#;
        let m = Manifest::parse(toml_src).expect("parse");
        assert!(m.models[0].role.is_post_processor());
        assert_eq!(m.models[0].role.to_string(), "post_processor");
    }

    /// An unknown spelling is refused at parse rather than silently read as the
    /// default — a typo'd role would otherwise put the model in the wrong slot.
    #[test]
    fn an_unknown_role_is_rejected() {
        let toml_src = r#"
[backend]
source = "github.com/x/y"
name = "Y"
version = "0.1.0"
kind = "wasm"
entrypoint = "y.wasm"
contract = "v2"
id = "app.super-stt.openai"
description = "Test backend."

[[models]]
name = "cleanup"
role = "postprocessor"
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["none"]
"#;
        assert!(Manifest::parse(toml_src).is_err());
    }

    /// A post-processor is handed a finished transcript over `POST /v1/process`
    /// and never sees the realtime path, so declaring both is a contradiction
    /// the daemon refuses at discovery.
    #[test]
    fn a_realtime_post_processor_is_rejected() {
        let toml_src = r#"
[backend]
source = "github.com/x/y"
name = "Y"
version = "0.1.0"
kind = "wasm"
entrypoint = "y.wasm"
contract = "v2"
id = "app.super-stt.openai"
description = "Test backend."

[capabilities]
websocket = true

[[models]]
name = "cleanup"
role = "post_processor"
realtime = true
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["none"]
"#;
        let m = Manifest::parse(toml_src).expect("parses; the rule is a runtime one");
        let err = validate_runtime(&m).expect_err("a realtime post-processor is refused");
        assert!(
            err.to_string().contains("post_processor"),
            "the message should name the contradiction: {err}"
        );
    }

    /// An option named after the collection's own route segment is refused at
    /// discovery.
    ///
    /// The regression this exists for is silent: `list` and `{name}` share a
    /// path segment, and the static route wins, so such an option appeared in
    /// the listing and could not be read, written or cleared — with nothing
    /// anywhere saying why. The name is what has to give, and it gives here,
    /// where a backend author sees the reason.
    #[test]
    fn an_option_named_after_a_reserved_segment_is_refused() {
        let toml_src = r#"
[backend]
source = "github.com/super-stt/openai"
name = "OpenAI"
version = "0.1.0"
kind = "wasm"
entrypoint = "openai.wasm"
contract = "v1"
description = "Test backend."

[[options]]
name = "list"
description = "Shadowed by the collection route."
"#;
        let manifest = Manifest::parse(toml_src).expect("parse");
        let err = validate_runtime(&manifest).expect_err("a reserved option name is refused");
        let text = err.to_string();
        assert!(text.contains("option `list`"), "names the offender: {text}");
        assert!(
            text.contains("unreachable"),
            "says what would happen, not just that it is refused: {text}"
        );
    }

    /// The same rule for secrets, which share the shape and the hazard.
    #[test]
    fn a_secret_named_after_a_reserved_segment_is_refused() {
        let toml_src = r#"
[backend]
source = "github.com/super-stt/openai"
name = "OpenAI"
version = "0.1.0"
kind = "wasm"
entrypoint = "openai.wasm"
contract = "v1"
description = "Test backend."

[[secrets]]
name = "list"
description = "Shadowed by the collection route."
required = false
"#;
        let manifest = Manifest::parse(toml_src).expect("parse");
        let err = validate_runtime(&manifest).expect_err("a reserved secret name is refused");
        assert!(
            err.to_string().contains("secret `list`"),
            "names the offender: {err}"
        );
    }

    /// And a name that merely contains the reserved word is fine — the check is
    /// on the whole segment, since that is what routing matches.
    #[test]
    fn a_name_containing_a_reserved_word_is_fine() {
        let toml_src = r#"
[backend]
source = "github.com/super-stt/openai"
name = "OpenAI"
version = "0.1.0"
kind = "wasm"
entrypoint = "openai.wasm"
contract = "v1"
description = "Test backend."

[[options]]
name = "allow_list"
description = "Not a collision."
"#;
        let manifest = Manifest::parse(toml_src).expect("parse");
        validate_runtime(&manifest).expect("allow_list is addressable");
    }
}
