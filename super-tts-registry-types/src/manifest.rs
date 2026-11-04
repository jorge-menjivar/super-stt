// SPDX-License-Identifier: GPL-3.0-only
//! `backend.toml`, as Super TTS reads it: everything in
//! [`super_engine_spec::manifest`], with the types that carry product fields
//! bound to [`Tts`].

pub use super_engine_spec::manifest::*;

use crate::product::Tts;
pub use crate::product::{
    Contract, TtsCapabilities, TtsManifestError, TtsModel, VoiceEntry, VoiceKind,
};

/// A Super TTS backend's `backend.toml`.
pub type Manifest = super_engine_spec::manifest::Manifest<Tts>;
/// `[backend]`, declaring a Super TTS [`Contract`].
pub type BackendMeta = super_engine_spec::manifest::BackendMeta<Contract>;
/// `[capabilities]`, with Super TTS's `streaming_input`.
pub type Capabilities = super_engine_spec::manifest::Capabilities<TtsCapabilities>;
/// One `[[models]]` entry, with Super TTS's voice and synthesis fields.
pub type ModelEntry = super_engine_spec::manifest::ModelEntry<TtsModel>;

/// Every field rule a Super TTS generation after v1 introduces.
pub const CONTRACT_FIELDS: &[ContractField<Contract>] =
    <Tts as super_engine_spec::product::Product>::CONTRACT_FIELDS;
