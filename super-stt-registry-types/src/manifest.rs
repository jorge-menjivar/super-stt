// SPDX-License-Identifier: GPL-3.0-only
//! `backend.toml`, as Super STT reads it: everything in
//! [`super_engine_spec::manifest`], with the types that carry product fields
//! bound to [`Stt`].

pub use super_engine_spec::manifest::*;

use crate::product::Stt;
pub use crate::product::{Contract, ModelRole, SttCapabilities, SttModel};

/// A Super STT backend's `backend.toml`.
pub type Manifest = super_engine_spec::manifest::Manifest<Stt>;
/// `[backend]`, declaring a Super STT [`Contract`].
pub type BackendMeta = super_engine_spec::manifest::BackendMeta<Contract>;
/// `[capabilities]`, with Super STT's `context`.
pub type Capabilities = super_engine_spec::manifest::Capabilities<SttCapabilities>;
/// One `[[models]]` entry, with Super STT's `role` and
/// `force_preview_support`.
pub type ModelEntry = super_engine_spec::manifest::ModelEntry<SttModel>;

/// Every field rule a Super STT generation after v1 introduces.
pub const CONTRACT_FIELDS: &[ContractField<Contract>] =
    <Stt as super_engine_spec::product::Product>::CONTRACT_FIELDS;
