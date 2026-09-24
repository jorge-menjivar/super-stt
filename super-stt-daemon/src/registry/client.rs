// SPDX-License-Identifier: GPL-3.0-only
//! Fetch and cache the registry's `index.json`:
//! `super_engine_daemon::registry::client`, shared with Super TTS, for Super
//! STT's index.

pub use super_engine_daemon::registry::client::{ClientError, DEFAULT_TTL};

/// Super STT's registry client.
pub type Client =
    super_engine_daemon::registry::client::Client<super_stt_registry_types::product::SttIndexModel>;
