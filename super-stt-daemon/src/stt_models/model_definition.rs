// SPDX-License-Identifier: GPL-3.0-only
//! Resolved model identity used throughout the daemon:
//! `super_engine_daemon::backends::ModelDefinition`, shared with Super TTS,
//! carrying Super STT's own `[[models]]` keys under `product` — what the
//! model is for (`product.role`) and whether it forces live previews
//! (`product.force_preview_support`).

/// Fully resolved description of a single model served by a backend. See
/// `super_engine_daemon::backends::ModelDefinition`.
pub type ModelDefinition =
    super_engine_daemon::backends::ModelDefinition<super_stt_registry_types::manifest::SttModel>;
