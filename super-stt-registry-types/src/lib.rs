// SPDX-License-Identifier: GPL-3.0-only
//! Canonical types for the Super STT registry contract: a backend's
//! `backend.toml` manifest and the maintainer-facing `registry.toml`.
//! See `docs/protocol/backend/config.md`.

pub mod entry;
pub mod forge;
pub mod license;
pub mod manifest;
pub mod provider;
mod safe_path;
#[cfg(feature = "schema")]
pub mod schema;

pub use safe_path::{is_safe_component, is_safe_relative_path};
