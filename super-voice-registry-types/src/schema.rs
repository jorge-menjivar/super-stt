// SPDX-License-Identifier: GPL-3.0-only
//! The JSON Schemas for Super STT's `backend.toml` and `registry.toml`, built
//! by [`super_engine_spec::schema`] from [`Voice`]'s contract.

use serde_json::Value;
use super_engine_spec::schema;

use crate::product::Voice;

/// The `backend.toml` schema.
#[must_use]
pub fn backend_schema() -> Value {
    schema::backend_schema::<Voice>()
}

/// The `registry.toml` schema.
#[must_use]
pub fn registry_schema() -> Value {
    schema::registry_schema::<Voice>()
}

/// [`backend_schema`], pretty-printed with a trailing newline.
#[must_use]
pub fn backend_schema_pretty() -> String {
    schema::backend_schema_pretty::<Voice>()
}

/// [`registry_schema`], pretty-printed with a trailing newline.
#[must_use]
pub fn registry_schema_pretty() -> String {
    schema::registry_schema_pretty::<Voice>()
}
