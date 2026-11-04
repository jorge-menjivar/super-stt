// SPDX-License-Identifier: GPL-3.0-only
//! `index.json`, as Super TTS reads it: everything in
//! [`super_engine_spec::index`], with the types that carry product fields
//! bound to Super TTS's [`TtsIndexModel`].

pub use super_engine_spec::index::*;

pub use crate::product::TtsIndexModel;

/// Super TTS's registry index.
pub type Index = super_engine_spec::index::Index<TtsIndexModel>;
/// One backend in [`Index`].
pub type IndexBackend = super_engine_spec::index::IndexBackend<TtsIndexModel>;
/// One model of an [`IndexBackend`].
pub type IndexModel = super_engine_spec::index::IndexModel<TtsIndexModel>;
