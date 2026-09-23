// SPDX-License-Identifier: GPL-3.0-only
//! `index.json`, as Super STT reads it: everything in
//! [`super_engine_spec::index`], with the types that carry product fields
//! bound to Super STT's [`SttIndexModel`].

pub use super_engine_spec::index::*;

pub use crate::product::SttIndexModel;

/// Super STT's registry index.
pub type Index = super_engine_spec::index::Index<SttIndexModel>;
/// One backend in [`Index`].
pub type IndexBackend = super_engine_spec::index::IndexBackend<SttIndexModel>;
/// One model of an [`IndexBackend`], with Super STT's `role`.
pub type IndexModel = super_engine_spec::index::IndexModel<SttIndexModel>;
