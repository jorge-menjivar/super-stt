// SPDX-License-Identifier: GPL-3.0-only
//! The JSON Schemas for Super STT's `backend.toml` and `registry.toml`, built
//! by [`super_engine_spec::schema`] from [`Stt`]'s contract.

use serde_json::Value;
use super_engine_spec::schema;

use crate::product::Stt;

/// The `backend.toml` schema.
#[must_use]
pub fn backend_schema() -> Value {
    schema::backend_schema::<Stt>()
}

/// The `registry.toml` schema.
#[must_use]
pub fn registry_schema() -> Value {
    schema::registry_schema::<Stt>()
}

/// [`backend_schema`], pretty-printed with a trailing newline.
#[must_use]
pub fn backend_schema_pretty() -> String {
    schema::backend_schema_pretty::<Stt>()
}

/// [`registry_schema`], pretty-printed with a trailing newline.
#[must_use]
pub fn registry_schema_pretty() -> String {
    schema::registry_schema_pretty::<Stt>()
}
