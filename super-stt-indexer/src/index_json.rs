// SPDX-License-Identifier: GPL-3.0-only
//! `index.json` output schema. Mirrors the spec at
//! `docs/superpowers/specs/2026-05-29-backend-registry-design.md`.

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;
/// Soft floor: the minimum Super STT client (daemon) version expected to
/// understand this index. Older clients still use the registry but are warned
/// to update. Compared with standard semver precedence on the consumer side.
pub const MIN_CLIENT: &str = "0.1.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Index {
    pub schema_version: u32,
    pub generated_at: String,
    pub min_client: String,
    pub backends: Vec<IndexBackend>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexBackend {
    pub id: String,
    pub source: String,
    pub version: String,
    pub tag: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub license: String,
    pub kind: String,
    pub contract: String,
    pub entrypoint: String,
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    pub online: bool,
    pub supports_gpu: bool,
    pub supports_cpu: bool,
    pub models: Vec<IndexModel>,
    pub secrets: Vec<IndexSecret>,
    pub options: Vec<IndexOption>,
    pub assets: IndexAssets,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_stale: Option<IndexStale>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexModel {
    pub name: String,
    pub provider: String,
    pub supported_devices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexSecret {
    pub name: String,
    pub label: String,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexOption {
    pub name: String,
    pub label: String,
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IndexAssets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasm: Option<IndexAsset>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subprocess: Vec<IndexSubprocessAsset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexAsset {
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexSubprocessAsset {
    pub target: String,
    pub accel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cuda_major: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cuda_sm: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cudnn: bool,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStale {
    pub latest_attempted: String,
    pub tag: String,
    pub error: String,
    pub since: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_a_minimal_index() {
        let idx = Index {
            schema_version: SCHEMA_VERSION,
            generated_at: "2026-05-29T18:00:00Z".into(),
            min_client: MIN_CLIENT.into(),
            backends: vec![IndexBackend {
                id: "openai".into(),
                source: "github.com/x/y".into(),
                version: "1.0.0".into(),
                tag: "v1.0.0".into(),
                name: "OpenAI".into(),
                description: None,
                license: "Apache-2.0".into(),
                kind: "wasm".into(),
                contract: "v1".into(),
                entrypoint: "openai.wasm".into(),
                allowed_hosts: vec!["api.openai.com".into()],
                online: true,
                supports_gpu: false,
                supports_cpu: false,
                models: vec![],
                secrets: vec![],
                options: vec![],
                assets: IndexAssets {
                    wasm: Some(IndexAsset {
                        url: "https://x".into(),
                        size: 1,
                        sha256: "abc".into(),
                    }),
                    subprocess: vec![],
                },
                index_stale: None,
            }],
        };
        let s = serde_json::to_string_pretty(&idx).unwrap();
        let back: Index = serde_json::from_str(&s).unwrap();
        assert_eq!(back.backends.len(), 1);
        assert_eq!(back.backends[0].id, "openai");
    }
}
