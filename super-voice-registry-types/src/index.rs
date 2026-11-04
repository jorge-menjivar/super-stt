// SPDX-License-Identifier: GPL-3.0-only
//! `index.json`, as Super Voice reads it: everything in
//! [`super_engine_spec::index`], with the types that carry product fields
//! bound to Super Voice's [`VoiceIndexModel`].

pub use super_engine_spec::index::*;

pub use crate::product::VoiceIndexModel;

/// Super Voice's registry index.
pub type Index = super_engine_spec::index::Index<VoiceIndexModel>;

/// One backend in [`Index`].
pub type IndexBackend = super_engine_spec::index::IndexBackend<VoiceIndexModel>;

/// One model of an [`IndexBackend`], with its `role`.
pub type IndexModel = super_engine_spec::index::IndexModel<VoiceIndexModel>;
