// SPDX-License-Identifier: GPL-3.0-only
//! Resolved model identity shared across the daemon, clients, and protocol.
//!
//! [`ModelDefinition`] is the unified description of a single model a backend
//! serves. Models are no longer compiled into the daemon; they are discovered
//! at runtime from installed backends (each backend ships a `backend.toml`
//! declaring its `[[models]]`). The daemon builds a `ModelDefinition` for every
//! discovered model and uses it everywhere a fully resolved model is needed.
//!
//! ## Identity
//!
//! `(name, provider, source)` is the canonical wire-level identity:
//!
//! - `name` — the model's wire name (e.g. `whisper-1`, `voxtral-mini`).
//! - `provider` — the engine family / routing class ([`Provider`]).
//! - `source` — the repo id of the backend that serves the model, e.g.
//!   `github.com/super-stt/openai`. This replaces the old `SourceKind` enum:
//!   two backends can serve a model with the same `(name, provider)` and the
//!   `source` keeps them distinct.

use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::models::provider::Provider;

/// Fully resolved description of a single model served by a backend.
///
/// Built by the daemon from a discovered backend's `backend.toml` entry; not a
/// static catalog. `source` carries the serving backend's repo id.
#[derive(Clone, Debug, Serialize, Deserialize)]
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
    /// Devices the model can be loaded onto, in wire form (`cpu`, `cuda`,
    /// `metal`, or the sentinel `none` for remote/online models, which must be
    /// the only entry when present). Non-empty and validated at discovery.
    pub supported_devices: Vec<String>,
    /// Whether this model is reached over the realtime WebSocket path
    /// (`/v1/transcribe/realtime`) rather than batch `POST /v1/transcribe`.
    pub realtime: bool,
}

impl ModelDefinition {
    /// Whether the model is served by a remote API with no local compute —
    /// encoded by the `none` sentinel in `supported_devices` (the only entry
    /// when present). This is the single source of the online/local
    /// distinction; `provider` is a free-form label and carries no such meaning.
    #[must_use]
    pub fn is_online(&self) -> bool {
        self.supported_devices.iter().any(|d| d == "none")
    }
}
