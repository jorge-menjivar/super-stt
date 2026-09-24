// SPDX-License-Identifier: GPL-3.0-only
//! A backend's `backend.toml`. The manifest types and parser are canonical in
//! `super-stt-registry-types`; the runtime policy a daemon holds a manifest
//! to at discovery is `super_engine_daemon::backends::validate_runtime`,
//! shared with Super TTS. This module re-exports both.

pub use super_engine_daemon::backends::validate_runtime;
pub use super_stt_registry_types::manifest::*;

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

    /// The context capability is not transport-restricted, unlike `websocket`
    /// beside it — both headers ride the ordinary request every backend
    /// receives, so a subprocess backend reads them the same way. That is a
    /// deliberate deviation from the precedent next door, so it is pinned.
    #[test]
    fn capabilities_context_parses_on_either_transport() {
        for kind in ["wasm", "subprocess"] {
            let entrypoint = if kind == "wasm" { "tidy.wasm" } else { "tidy" };
            let toml_src = format!(
                r#"
[backend]
source = "github.com/super-stt/tidy"
name = "Tidy"
version = "0.2.0"
kind = "{kind}"
entrypoint = "{entrypoint}"
contract = "v1"
description = "Test backend."

[capabilities]
context = true
"#
            );
            let m = Manifest::parse(&toml_src).unwrap_or_else(|e| panic!("parse {kind}: {e}"));
            assert!(m.capabilities.product.context);
            assert!(
                !m.capabilities.websocket,
                "one flag does not imply the other"
            );
            validate_runtime(&m).unwrap_or_else(|e| {
                panic!("a {kind} backend may declare the context capability: {e}")
            });
        }
    }

    #[test]
    fn capabilities_context_defaults_false_when_absent() {
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
        assert!(
            !m.capabilities.product.context,
            "absent means no context headers are sent"
        );
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
        assert_eq!(m.models[0].product.role, ModelRole::Transcription);
        assert!(!m.models[0].product.role.is_post_processor());
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
        assert!(m.models[0].product.role.is_post_processor());
        assert_eq!(m.models[0].product.role.to_string(), "post_processor");
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
}
