// SPDX-License-Identifier: GPL-3.0-only
//! Fetch and cache the registry's `index.json`:
//! `super_engine_daemon::registry::client`, shared with Super STT, for Super
//! TTS's index.

pub use super_engine_daemon::registry::client::{ClientError, DEFAULT_TTL};

/// Super TTS's registry client.
pub type Client =
    super_engine_daemon::registry::client::Client<super_tts_registry_types::product::TtsIndexModel>;
