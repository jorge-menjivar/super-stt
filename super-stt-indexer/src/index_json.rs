// SPDX-License-Identifier: GPL-3.0-only
//! `index.json` output schema. The canonical definitions now live in
//! `super-stt-registry-types::index` (shared with the daemon consumer and the
//! `/registry/backends` leaf types); this module re-exports them so the
//! indexer's `index_json::*` references keep working.

pub use super_stt_registry_types::index::*;
