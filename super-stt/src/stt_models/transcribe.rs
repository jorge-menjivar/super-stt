// SPDX-License-Identifier: GPL-3.0-only
//! Unified traits for all STT models — local, custom, and online.
//!
//! Three layered surfaces:
//! - [`ModelInfo`] — static metadata (name, provider, capabilities).
//! - [`ModelState`] — runtime state that can change after load (device).
//! - [`Transcribe`] — actual inference.
//!
//! `Transcribe` is a supertrait of `ModelState`, which is a supertrait of
//! `ModelInfo`, so a `Box<dyn Transcribe>` exposes all three.

use anyhow::Result;
use async_trait::async_trait;
use candle_core::Device;
use std::time::Duration;

use super_stt_shared::models::provider::Provider;
use super_stt_shared::models::registry::{self, ModelDefinition, SourceKind};

/// Static metadata about a loaded model.
///
/// For built-in models, the data comes straight from the registry. For custom
/// models, the daemon constructs a synthesized `ModelDefinition` at load time.
#[derive(Debug, Clone)]
pub struct ModelInfoData {
    /// Wire-level model name.
    pub name: String,
    /// Engine family + routing class.
    pub provider: Provider,
    /// Whether this is a built-in (registry hit) or custom (user disk).
    pub source: SourceKind,
    /// Pointer to the registry entry, if `source == Builtin` (or `Online`).
    pub definition: Option<&'static ModelDefinition>,
}

impl ModelInfoData {
    /// Build metadata for a built-in or online registry entry. Returns
    /// `None` if `(name, provider)` doesn't resolve to a registry entry.
    #[must_use]
    pub fn standard(name: &str, provider: Provider) -> Option<Self> {
        let def = registry::find_by(name, provider)?;
        Some(Self {
            name: name.to_string(),
            provider,
            source: def.source.kind(),
            definition: Some(def),
        })
    }

    /// Build metadata for a user-provided custom model.
    #[must_use]
    pub fn custom(name: String, provider: Provider) -> Self {
        Self {
            name,
            provider,
            source: SourceKind::Custom,
            definition: None,
        }
    }

    #[must_use]
    pub fn is_custom(&self) -> bool {
        matches!(self.source, SourceKind::Custom)
    }
}

/// Static metadata accessor surface — implemented by every loaded model.
pub trait ModelInfo: Send + Sync {
    /// The underlying metadata payload.
    fn info(&self) -> &ModelInfoData;

    /// Engine family + routing class.
    fn provider(&self) -> Provider {
        self.info().provider
    }

    /// Wire-level name.
    fn display_name(&self) -> &str {
        &self.info().name
    }

    /// Whether this model supports multiple languages.
    /// Custom models default to `true` (we don't know).
    fn is_multilingual(&self) -> bool {
        self.info().definition.is_none_or(|d| d.is_multilingual)
    }

    /// Whether this model is loaded from `custom_models_dir`.
    fn is_custom(&self) -> bool {
        self.info().is_custom()
    }

    /// Whether this model sends audio to an external API.
    fn is_online(&self) -> bool {
        matches!(self.info().provider, Provider::Online(_))
    }

    /// Suggested minimum interval between real-time processing chunks.
    fn processing_interval(&self) -> Duration {
        self.info()
            .definition
            .map_or(Duration::from_secs(2), |d| d.processing_interval)
    }
}

/// Runtime state that may change after the model is loaded — currently just
/// the device the model is bound to (which can flip from CUDA to CPU on
/// fallback). Online models report `&Device::Cpu` as a placeholder.
pub trait ModelState: ModelInfo {
    /// Device the model runs on. Online models return `&Device::Cpu` as a placeholder.
    fn device(&self) -> &Device;
}

/// Common contract for any STT backend.
///
/// Implementations may run inference synchronously (e.g. local models on
/// CPU/GPU) or asynchronously (e.g. online models hitting an API). The trait
/// is `async` so callers can treat both uniformly.
#[async_trait]
pub trait Transcribe: ModelState {
    /// Transcribe audio samples to plain text.
    async fn transcribe_audio(&mut self, audio: &[f32], sample_rate: u32) -> Result<String>;
}
