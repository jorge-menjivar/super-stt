// SPDX-License-Identifier: GPL-3.0-only
//! Resolved model identity used throughout the daemon:
//! `super_engine_daemon::backends::ModelDefinition`, shared with Super STT,
//! carrying Super TTS's own `[[models]]` keys under `product` — the voices a
//! model has (`product.voices`, `product.voice_kinds`,
//! `product.default_voice`), how it clones one, and the longest text it takes
//! in one request (`product.max_input_chars`).

/// Fully resolved description of a single model served by a backend. See
/// `super_engine_daemon::backends::ModelDefinition`.
pub type ModelDefinition =
    super_engine_daemon::backends::ModelDefinition<super_tts_registry_types::manifest::TtsModel>;
