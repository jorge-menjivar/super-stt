// SPDX-License-Identifier: GPL-3.0-only
//! Super TTS's side of the backend contract: its contract generations and the
//! voice and synthesis fields its manifests add ([`product`]), on top of the
//! contract it shares with Super STT in `super-engine-spec`.
//!
//! The modules mirror `super-engine-spec`'s, with every type that carries
//! product fields bound to [`Tts`], so the rest of the workspace never names
//! the product. See `docs/protocol/backend/config.md`.

pub mod index;
pub mod manifest;
pub mod product;
#[cfg(feature = "schema")]
pub mod schema;

pub use product::Tts;
pub use super_engine_spec::{
    arch, backend_id, entry, forge, fs, is_safe_component, is_safe_relative_path, license, verify,
    version,
};
