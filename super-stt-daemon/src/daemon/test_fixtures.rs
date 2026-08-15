// SPDX-License-Identifier: GPL-3.0-only
//! Backend fixtures shared across the daemon's test modules.
//!
//! One builder per shape, so a field added to `DiscoveredBackend` or `Opt`
//! breaks a single definition instead of a hand-written literal in every module
//! that happens to need a backend.

use crate::stt_models::ModelDefinition;
use crate::stt_models::backends::DiscoveredBackend;
use crate::stt_models::backends::manifest::{Opt, OptionDefault, OptionType, Secret};

/// The cloud-backend shape the option, egress, and catalog tests all need: one
/// required secret, `api.openai.com` as declared egress, and a `base_url`
/// option.
///
/// `base_url_default` is normally `None` — a manifest may not declare a default
/// for that option, and [`Manifest::parse`] rejects one. Pass `Some` only to
/// build the state the parser forbids, where the point of the test is that the
/// daemon does not rely on that rejection.
///
/// [`Manifest::parse`]: super_stt_registry_types::manifest::Manifest::parse
pub(crate) fn openai_backend(
    source: &str,
    models: Vec<ModelDefinition>,
    base_url_default: Option<&str>,
) -> DiscoveredBackend {
    DiscoveredBackend {
        dir: std::path::PathBuf::from("/tmp/openai"),
        source: source.to_string(),
        name: "OpenAI".to_string(),
        kind: "wasm".to_string(),
        entrypoint: "openai.wasm".to_string(),
        allowed_hosts: vec!["api.openai.com".to_string()],
        secrets: vec![Secret {
            name: "openai_api_key".to_string(),
            label: Some("OpenAI API key".to_string()),
            description: "key".to_string(),
            required: true,
        }],
        options: vec![Opt {
            name: "base_url".to_string(),
            label: Some("API base URL".to_string()),
            description: "Base URL".to_string(),
            r#type: Some(OptionType::String),
            default: base_url_default.map(|d| OptionDefault::String(d.to_string())),
            required: false,
        }],
        models,
    }
}
