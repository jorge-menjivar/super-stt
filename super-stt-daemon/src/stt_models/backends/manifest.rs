// SPDX-License-Identifier: GPL-3.0-only
//! Parsing of a backend's `backend.toml`. See `docs/protocol/backend/config.md`.
//!
//! This is the single canonical manifest parser, shared by backend discovery
//! and the WASM / subprocess hosts.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub backend: BackendMeta,
    #[serde(default)]
    pub network: Network,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub secrets: Vec<Secret>,
    #[serde(default)]
    pub options: Vec<Opt>,
    #[serde(default)]
    pub models: Vec<ModelEntry>,
}

#[derive(Debug, Deserialize)]
pub struct BackendMeta {
    /// Repo id, e.g. `github.com/super-stt/openai`. Used as the model `source`.
    pub source: String,
    pub name: String,
    pub version: String,
    /// `"wasm"` or `"subprocess"`.
    pub kind: String,
    pub entrypoint: String,
    pub contract: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct Network {
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Capabilities {
    /// Opt into the `super-stt:realtime/ws` import + `ws-server` export.
    /// Only meaningful for wasm backends; subprocess backends declaring this
    /// are rejected at discovery (see `Manifest::validate`).
    #[serde(default)]
    pub websocket: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Secret {
    /// `snake_case` identifier the backend reads as `x-stt-secret-<name>`.
    pub name: String,
    /// Human-readable label for the UI (e.g. `"OpenAI API key"`). Falls back to
    /// `name` when absent.
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Opt {
    /// `snake_case` identifier the backend reads as `x-stt-option-<name>`.
    pub name: String,
    /// Human-readable label for the UI (e.g. `"API base URL"`). Falls back to
    /// `name` when absent.
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub default: Option<toml::Value>,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Deserialize)]
pub struct ModelEntry {
    pub name: String,
    pub provider: String,
    #[serde(default = "default_true")]
    pub multilingual: bool,
    #[serde(default)]
    pub primary_language: Option<String>,
    #[serde(default)]
    pub supported_languages: Vec<String>,
    /// Devices the model can be loaded onto. `snake_case` values from
    /// `["cpu", "cuda", "metal", "none"]`; the sentinel `"none"` is for
    /// remote/online models with no local compute and must be the only entry
    /// when present. Required — a backend that omits or empties this field
    /// is rejected at discovery.
    #[serde(default)]
    pub supported_devices: Vec<String>,
    #[serde(default)]
    pub estimated_vram_bytes: u64,
    #[serde(default)]
    pub processing_interval_ms: Option<u64>,
    /// When `true`, the model is reached over WebSocket end-to-end.
    #[serde(default)]
    pub realtime: bool,
    /// Files to provision before the backend runs (subprocess backends).
    #[serde(default)]
    pub files: Vec<FilesSpec>,
}

#[derive(Debug, Deserialize)]
pub struct FilesSpec {
    #[serde(default = "default_hf_source")]
    pub source: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default = "default_revision")]
    pub revision: String,
    #[serde(default)]
    pub files: Vec<String>,
    pub dest: String,
}

fn default_true() -> bool {
    true
}

fn default_revision() -> String {
    "main".to_string()
}

fn default_hf_source() -> String {
    "huggingface".to_string()
}

impl Manifest {
    /// Read and parse `backend.toml` from a backend directory.
    ///
    /// # Errors
    /// Returns an error if the file is missing or fails to parse.
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join("backend.toml");
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let m: Self = toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        // The entrypoint is joined onto the backend dir to spawn/load the
        // backend; an absolute or traversing value would escape it. Reject at
        // the single canonical loader so every host inherits the guard.
        anyhow::ensure!(
            super_stt_shared::registry::is_safe_component(&m.backend.entrypoint),
            "backend.toml entrypoint {:?} is not a safe relative path component",
            m.backend.entrypoint
        );
        Ok(m)
    }

    /// Validate cross-field invariants that serde can't enforce on its own.
    ///
    /// # Errors
    /// Returns an error if any invariant is violated.
    pub fn validate(&self) -> Result<()> {
        if self.backend.kind == "subprocess" && self.capabilities.websocket {
            anyhow::bail!(
                "[capabilities].websocket is wasm-only; subprocess backends cannot declare it"
            );
        }
        for model in &self.models {
            if model.realtime && !self.capabilities.websocket {
                anyhow::bail!(
                    "model `{}` has realtime = true but capabilities.websocket is not set",
                    model.name
                );
            }
        }
        Ok(())
    }
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

[[secrets]]
name = "openai_api_key"
label = "OpenAI API key"
description = "Authenticate requests."
required = true
"#;
        let manifest: Manifest = toml::from_str(toml_src).expect("parse");
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

[[secrets]]
name = "openai_api_key"
description = "Authenticate requests."
"#;
        let manifest: Manifest = toml::from_str(toml_src).expect("parse");
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

[capabilities]
websocket = true
"#;
        let m: Manifest = toml::from_str(toml_src).expect("parse");
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
"#;
        let m: Manifest = toml::from_str(toml_src).expect("parse");
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

[capabilities]
websocket = true

[[models]]
name = "voxtral-mini-transcribe-realtime-2602"
provider = "mistral"
multilingual = true
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["none"]
realtime = true
"#;
        let m: Manifest = toml::from_str(toml_src).expect("parse");
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

[[models]]
name = "voxtral-mini-latest"
provider = "mistral"
multilingual = true
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["none"]
"#;
        let m: Manifest = toml::from_str(toml_src).expect("parse");
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

[capabilities]
websocket = true
"#;
        let m: Manifest = toml::from_str(toml_src).expect("parse");
        let err = m.validate().expect_err("subprocess + websocket must fail");
        assert!(err.to_string().contains("wasm-only"), "got: {err}");
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

[[models]]
name = "voxtral-mini-transcribe-realtime-2602"
provider = "mistral"
multilingual = true
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["none"]
realtime = true
"#;
        let m: Manifest = toml::from_str(toml_src).expect("parse");
        let err = m
            .validate()
            .expect_err("realtime model without websocket capability must fail");
        assert!(
            err.to_string().contains("capabilities.websocket"),
            "got: {err}"
        );
    }

    /// Options likewise carry an optional `label`; presence and absence both
    /// parse, and the default value/`kind` survive the round-trip.
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
        let manifest: Manifest = toml::from_str(toml_src).expect("parse");
        assert_eq!(manifest.options.len(), 2);

        let base = &manifest.options[0];
        assert_eq!(base.name, "base_url");
        assert_eq!(base.label.as_deref(), Some("API base URL"));
        assert_eq!(base.kind.as_deref(), Some("string"));
        assert_eq!(
            base.default.as_ref().and_then(toml::Value::as_str),
            Some("https://api.openai.com")
        );

        let timeout = &manifest.options[1];
        assert!(timeout.label.is_none(), "label is optional");
        assert_eq!(timeout.kind.as_deref(), Some("integer"));
        assert_eq!(
            timeout.default.as_ref().and_then(toml::Value::as_integer),
            Some(30)
        );
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
"#,
        )
        .unwrap();
        let err = Manifest::load(dir.path()).expect_err("absolute entrypoint must be rejected");
        assert!(err.to_string().contains("entrypoint"), "got: {err}");
    }
}
