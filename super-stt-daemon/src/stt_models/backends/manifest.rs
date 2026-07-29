// SPDX-License-Identifier: GPL-3.0-only
//! Runtime-policy validation of a backend's `backend.toml`. The manifest
//! types and parser are canonical in `super-stt-registry-types`; this module
//! re-exports them and adds the checks only the daemon cares about.

use anyhow::Result;

pub use super_stt_registry_types::manifest::*;

/// Validate cross-field invariants the daemon enforces at discovery.
///
/// # Errors
/// Returns an error if a subprocess backend declares the wasm-only
/// `websocket` capability or a non-empty `allowed_hosts` (the transport
/// provides no network), if a model's `primary_language` is absent from its
/// `supported_languages`, if a non-multilingual model's `supported_languages`
/// is not exactly `[primary_language]`, or if a model sets `realtime` without
/// the `websocket` capability.
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
name = "base_url"
label = "API base URL"
description = "Override the base URL."
type = "string"
default = "https://api.openai.com"

[[options]]
name = "request_timeout_seconds"
description = "Per-request timeout."
type = "integer"
default = 30
"#;
        let manifest = Manifest::parse(toml_src).expect("parse");
        assert_eq!(manifest.options.len(), 2);

        let base = &manifest.options[0];
        assert_eq!(base.name, "base_url");
        assert_eq!(base.label.as_deref(), Some("API base URL"));
        assert_eq!(base.r#type, Some(OptionType::String));
        assert_eq!(
            base.default,
            Some(OptionDefault::String("https://api.openai.com".into()))
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
}
