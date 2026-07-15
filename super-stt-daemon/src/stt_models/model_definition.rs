// SPDX-License-Identifier: GPL-3.0-only
//! Resolved model identity used throughout the daemon.
//!
//! [`ModelDefinition`] is the unified description of a single model a backend
//! serves. Models are not compiled into the daemon; they are discovered at
//! runtime from installed backends (each backend ships a `backend.toml`
//! declaring its `[[models]]`). The daemon builds a `ModelDefinition` for every
//! discovered model and uses it everywhere a fully resolved model is needed.
//! This type is daemon-internal — it never crosses the wire (client-facing
//! model metadata flows through the protocol types in `super-stt-shared`).
//!
//! ## Identity
//!
//! `(name, provider, source)` is the canonical wire-level identity:
//!
//! - `name` — the model's wire name (e.g. `whisper-1`, `voxtral-mini`).
//! - `provider` — the engine family / routing class ([`Provider`]).
//! - `source` — the repo id of the backend that serves the model, e.g.
//!   `github.com/super-stt/openai`. Two backends can serve a model with the same
//!   `(name, provider)` and the `source` keeps them distinct.

use std::time::Duration;

use super_stt_registry_types::manifest::Device;
use super_stt_shared::models::provider::Provider;

/// Fully resolved description of a single model served by a backend.
///
/// Built by the daemon from a discovered backend's `backend.toml` entry; not a
/// static catalog. `source` carries the serving backend's repo id.
#[derive(Clone, Debug)]
pub struct ModelDefinition {
    /// Wire-level model name.
    pub name: String,
    /// Engine family + routing class.
    pub provider: Provider,
    /// Repo id of the backend that serves this model (e.g.
    /// `github.com/super-stt/openai`).
    pub source: String,
    /// Whether the model supports multiple languages.
    pub is_multilingual: bool,
    /// The model's default language (BCP-47), used when no language is sent.
    pub primary_language: String,
    /// Language tags the model accepts (base and/or region-qualified).
    pub supported_languages: Vec<String>,
    /// Conservative GPU memory estimate including weights, KV cache, and
    /// overhead. `0` when unknown or not GPU-resident.
    pub estimated_vram_bytes: u64,
    /// Suggested minimum interval between real-time processing chunks.
    pub processing_interval: Duration,
    /// Devices the model can be loaded onto. The sentinel [`Device::None`]
    /// (remote/online model with no local compute) must be the only entry when
    /// present. Non-empty and validated at discovery.
    pub supported_devices: Vec<Device>,
    /// Whether this model is reached over the realtime WebSocket path
    /// (`/v1/transcribe/realtime`) rather than batch `POST /v1/transcribe`.
    pub realtime: bool,
}

impl ModelDefinition {
    /// Whether the model is served by a remote API with no local compute —
    /// encoded by the [`Device::None`] sentinel in `supported_devices` (the only
    /// entry when present). This is the single source of the online/local
    /// distinction; `provider` is a free-form label and carries no such meaning.
    #[must_use]
    pub fn is_online(&self) -> bool {
        self.supported_devices.contains(&Device::None)
    }
}
