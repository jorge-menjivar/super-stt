// SPDX-License-Identifier: GPL-3.0-only
//! Re-export of the canonical provider types. They live in
//! `super-stt-registry-types` so the registry indexer can use them without
//! depending on this crate; existing `super_stt_shared::models::provider::*`
//! paths keep working through this re-export.
pub use super_stt_registry_types::provider::{OnlineProvider, Provider};
