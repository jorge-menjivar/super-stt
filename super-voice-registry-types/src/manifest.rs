// SPDX-License-Identifier: GPL-3.0-only
//! `backend.toml`, as Super Voice reads it: everything in
//! [`super_engine_spec::manifest`], with the types that carry product fields
//! bound to [`Voice`].

pub use super_engine_spec::manifest::*;

use crate::product::Voice;
pub use crate::product::{
    Contract, ModelRole, VoiceCapabilities, VoiceEntry, VoiceKind, VoiceManifestError, VoiceModel,
};

/// A Super Voice backend's `backend.toml`.
pub type Manifest = super_engine_spec::manifest::Manifest<Voice>;

/// `[backend]`, declaring a Super Voice [`Contract`].
pub type BackendMeta = super_engine_spec::manifest::BackendMeta<Contract>;

/// `[capabilities]`, with Super Voice's `context` and `streaming_input`.
pub type Capabilities = super_engine_spec::manifest::Capabilities<VoiceCapabilities>;

/// One `[[models]]` entry, with its `role` and the keys of its feature.
pub type ModelEntry = super_engine_spec::manifest::ModelEntry<VoiceModel>;

/// Every field rule a Super Voice generation introduces.
pub const CONTRACT_FIELDS: &[ContractField<Contract>] =
    <Voice as super_engine_spec::product::Product>::CONTRACT_FIELDS;
