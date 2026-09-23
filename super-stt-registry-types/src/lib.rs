// SPDX-License-Identifier: GPL-3.0-only
//! Super STT's side of the backend contract: its contract generations and the
//! fields its manifests and index add ([`product`]), on top of the contract it
//! shares with Super TTS in `super-engine-spec`.
//!
//! The modules mirror `super-engine-spec`'s, with every type that carries
//! product fields bound to [`Stt`], so the rest of the workspace never names
//! the product. See `docs/protocol/backend/config.md`.

pub mod index;
pub mod manifest;
pub mod product;
#[cfg(feature = "schema")]
pub mod schema;

pub use product::Stt;
pub use super_engine_spec::{
    arch, backend_id, entry, forge, fs, is_safe_component, is_safe_relative_path, license, verify,
    version,
};
