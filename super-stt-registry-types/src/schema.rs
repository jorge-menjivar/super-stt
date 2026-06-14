// SPDX-License-Identifier: GPL-3.0-only
//! JSON Schema generation for `backend.toml` and `registry.toml`.
//!
//! Structure, field names, requiredness, enums, and descriptions are derived
//! from the Rust types. Two things are injected here because no struct can
//! express them: cross-field conditionals (kind→assets shape, accel→cuda
//! fields) and `additionalProperties: false` (the
//! parsers stay lenient for forward compatibility; the editor schema is
//! strict to catch typos). Draft-07 output for taplo compatibility.

use serde_json::{Value, json};

const BACKEND_SCHEMA_ID: &str = "https://jorge-menjivar.github.io/super-stt/backend.schema.json";
const REGISTRY_SCHEMA_ID: &str = "https://jorge-menjivar.github.io/super-stt/registry.schema.json";
const SPDX: &str = "SPDX-License-Identifier: GPL-3.0-only";

fn draft07_value<T: schemars::JsonSchema>() -> Value {
    let schema = schemars::generate::SchemaSettings::draft07()
        .into_generator()
        .into_root_schema_for::<T>();
    serde_json::to_value(schema).expect("schema serializes")
}

/// Set `additionalProperties: false` on every object schema (root + each
/// definition) that declares `properties` and doesn't already set it.
fn close_objects(v: &mut Value) {
    fn close_one(obj: &mut serde_json::Map<String, Value>) {
        if obj.contains_key("properties") && !obj.contains_key("additionalProperties") {
            obj.insert("additionalProperties".into(), json!(false));
        }
    }
    if let Some(obj) = v.as_object_mut() {
        close_one(obj);
        if let Some(defs) = obj.get_mut("definitions").and_then(Value::as_object_mut) {
            for def in defs.values_mut() {
                if let Some(d) = def.as_object_mut() {
                    close_one(d);
                }
            }
        }
    }
}

/// Push `cond` onto the schema object's `allOf`, creating it if needed.
fn push_all_of(schema_obj: &mut Value, cond: Value) {
    let obj = schema_obj.as_object_mut().expect("object schema");
    obj.entry("allOf")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .expect("allOf array")
        .push(cond);
}

/// The full `backend.toml` schema.
///
/// # Panics
/// Panics if the schemars output is missing an expected definition or is not
/// an object schema — both mean a schemars upgrade changed the output shape
/// and this builder must be updated, not silently skip its conditionals.
#[must_use]
pub fn backend_schema() -> Value {
    let mut root = draft07_value::<crate::manifest::Manifest>();

    {
        let obj = root.as_object_mut().expect("root object");
        obj.insert("$id".into(), json!(BACKEND_SCHEMA_ID));
        obj.insert("$comment".into(), json!(SPDX));
        obj.insert(
            "title".into(),
            json!("Super STT backend manifest (backend.toml)"),
        );
    }

    // kind → assets shape, enforced only when [assets] is present (local
    // installs legitimately omit it; the indexer requires it at publish).
    push_all_of(
        &mut root,
        json!({
            "if": {
                "required": ["backend", "assets"],
                "properties": { "backend": {
                    "required": ["kind"],
                    "properties": { "kind": { "const": "wasm" } }
                } }
            },
            "then": { "properties": { "assets": { "required": ["wasm"] } } }
        }),
    );
    push_all_of(
        &mut root,
        json!({
            "if": {
                "required": ["backend", "assets"],
                "properties": { "backend": {
                    "required": ["kind"],
                    "properties": { "kind": { "const": "subprocess" } }
                } }
            },
            "then": { "properties": { "assets": {
                "required": ["subprocess"],
                "properties": { "subprocess": { "minItems": 1 } }
            } } }
        }),
    );

    // Publication intent ⇒ [assets] present ⇒ license must be declared. Mirrors
    // the kind→assets rule above: a locally installed backend legitimately omits
    // both [assets] and the license; a release that declares assets must name a
    // license. `license`'s *value* is constrained by the enum injected below.
    push_all_of(
        &mut root,
        json!({
            "if": { "required": ["assets"] },
            "then": { "properties": { "backend": { "required": ["license"] } } }
        }),
    );

    // Per-definition conditionals.
    let defs = root
        .get_mut("definitions")
        .and_then(Value::as_object_mut)
        .expect("definitions");
    let asset = defs
        .get_mut("SubprocessAsset")
        .expect("SubprocessAsset def");
    push_all_of(
        asset,
        json!({
            "if": {
                "required": ["accel"],
                "properties": { "accel": { "const": "cuda" } }
            },
            "then": { "required": ["cuda_major"] },
            "else": { "properties": {
                "cuda_major": false,
                "cuda_sm": false,
                "cudnn": { "const": false }
            } }
        }),
    );
    // `FileSpec` is flat — `url` and `destination` are required by serde and
    // `sha256` is optional, so no cross-field conditional is needed here.

    // `supported_devices` must be non-empty (discovery rejects an empty
    // list); serde can't express minItems, so inject it here.
    let model = defs.get_mut("ModelEntry").expect("ModelEntry def");
    model["properties"]["supported_devices"]
        .as_object_mut()
        .expect("supported_devices property")
        .insert("minItems".into(), serde_json::json!(1));

    // Embed the accepted license values (recognized FOSS SPDX ids + `other`) as
    // an enum so editors offer them and reject anything else — a self-contained
    // snapshot of the FOSS subset of the SPDX list, requiring no external fetch
    // by the editor. Built from the same predicate the indexer validates with
    // (`crate::license`), so the schema and the indexer never disagree.
    let licenses: Vec<Value> = crate::license::accepted_schema_values()
        .into_iter()
        .map(|s| json!(s))
        .collect();
    let backend = defs.get_mut("BackendMeta").expect("BackendMeta def");
    backend["properties"]["license"]
        .as_object_mut()
        .expect("license property")
        .insert("enum".into(), json!(licenses));

    close_objects(&mut root);
    root
}

/// The full `registry.toml` schema: a map of backend-id → entry.
///
/// # Panics
/// Panics if the schemars output for [`crate::entry::Entry`] does not
/// serialize — see [`backend_schema`].
#[must_use]
pub fn registry_schema() -> Value {
    let mut entry = draft07_value::<crate::entry::Entry>();
    if let Some(obj) = entry.as_object_mut() {
        obj.remove("$schema");
        obj.remove("$id");
        obj.remove("title");
    }
    close_objects(&mut entry);

    // `Entry` is all scalar fields today, so its schema embeds cleanly. If it
    // ever gains a $ref'd nested type, that type's definition would need
    // hoisting into this root's `definitions` — the validator_for() call in
    // the tests fails loudly on the dangling ref if this assumption breaks.
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": REGISTRY_SCHEMA_ID,
        "$comment": SPDX,
        "title": "Super STT backend registry",
        "description": "Source of truth for the installable-backend catalog. Each top-level table is one backend, keyed by a unique backend id (ascii lowercase, digits, `-`, `_`). A scheduled GitHub Action reads this file, resolves each entry's latest GitHub release, validates it, and publishes index.json to the gh-pages branch. See registry/README.md for submission rules.",
        "type": "object",
        "additionalProperties": false,
        "patternProperties": { "^[a-z0-9_-]+$": { "$ref": "#/definitions/entry" } },
        "definitions": { "entry": entry }
    })
}

/// Pretty-printed schema text exactly as written to disk — the generator and
/// the drift test share this single serialization path.
///
/// # Panics
/// Panics when [`backend_schema`] does.
#[must_use]
pub fn backend_schema_pretty() -> String {
    let mut s = serde_json::to_string_pretty(&backend_schema()).expect("serializes");
    s.push('\n');
    s
}

/// Pretty-printed registry schema text; see [`backend_schema_pretty`].
///
/// # Panics
/// Panics when [`registry_schema`] does.
#[must_use]
pub fn registry_schema_pretty() -> String {
    let mut s = serde_json::to_string_pretty(&registry_schema()).expect("serializes");
    s.push('\n');
    s
}
