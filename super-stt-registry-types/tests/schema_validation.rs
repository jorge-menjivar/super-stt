// SPDX-License-Identifier: GPL-3.0-only
//! The generated schema must accept every in-repo manifest, and must encode
//! Super STT's own contract table. The rules every product shares are tested
//! in `super-engine-spec`.
#![cfg(feature = "schema")]

use serde_json::{Value, json};

fn backend_validator() -> jsonschema::Validator {
    jsonschema::validator_for(&super_stt_registry_types::schema::backend_schema())
        .expect("backend schema compiles")
}

fn toml_to_json(text: &str) -> Value {
    toml::from_str(text).expect("valid TOML")
}

/// Every backend manifest shipped in-repo must match the published schema.
/// This is what catches the schema drifting away from manifests people
/// actually write, so it has to *have* inputs: it previously scanned
/// `backends/`, which no longer exists, and passed while validating nothing.
/// The count assertion at the end is what stops that recurring silently.
#[test]
fn accepts_every_in_repo_backend_toml() {
    let v = backend_validator();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("super-stt-daemon/tests/fixtures");
    let entries = std::fs::read_dir(&dir).expect("daemon test fixtures directory");

    let mut checked = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        // Only manifests: the `Cargo.toml`s of the mock backends live in
        // subdirectories, so a non-recursive `*backend.toml` match skips them.
        if !path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with("backend.toml"))
        {
            continue;
        }
        let doc = toml_to_json(&std::fs::read_to_string(&path).unwrap());
        let errors: Vec<String> = v.iter_errors(&doc).map(|e| format!("{e}")).collect();
        assert!(
            errors.is_empty(),
            "{} schema errors: {errors:#?}",
            path.display()
        );
        checked += 1;
    }

    assert!(
        checked > 0,
        "no backend manifests found under {} — this test would pass while validating nothing",
        dir.display()
    );
}

/// The in-repo fixtures are the only manifests this crate can reach, so they
/// are also the only thing keeping the shipped shape covered. Every published
/// backend declares `provider` on its models; if no fixture does, the
/// acceptance test above stops exercising the one key most likely to be
/// dropped from the schema by accident.
#[test]
fn a_fixture_still_exercises_the_provider_key() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("super-stt-daemon/tests/fixtures");
    let covered = std::fs::read_dir(&dir)
        .expect("daemon test fixtures directory")
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.ends_with("backend.toml"))
        })
        .any(|e| {
            toml_to_json(&std::fs::read_to_string(e.path()).unwrap())
                .get("models")
                .and_then(Value::as_array)
                .is_some_and(|models| models.iter().any(|m| m.get("provider").is_some()))
        });
    assert!(
        covered,
        "no fixture under {} declares a model `provider` — the schema's tolerance of the \
         key every published manifest carries is no longer covered",
        dir.display()
    );
}

/// The schema requires an `id` of every entry. Three predate the requirement
/// and cannot be fixed from this file: an entry that declares an `id` pins the
/// release to it, and none of those three backends publishes a manifest
/// declaring one — so adding it here would turn a missing id into an
/// `IdMismatch` and drop the backend out of the catalog. Fixing one means
/// adding `[backend].id` to that backend's own repo, cutting a release, and
/// only then editing this file.
///
/// They are named here so the backlog is visible and shrinking: delete a name
/// when its entry gains an `id`. Anything *else* the schema objects to — a new
/// entry with no id, a malformed key — fails this test.
const ENTRIES_STILL_WITHOUT_AN_ID: &[&str] = &["deepgram", "qwen3_asr", "voxtral"];

#[test]
fn accepts_registry_toml() {
    let v = jsonschema::validator_for(&super_stt_registry_types::schema::registry_schema())
        .expect("registry schema compiles");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let doc = toml_to_json(&std::fs::read_to_string(root.join("registry/registry.toml")).unwrap());

    let mut missing_id: Vec<String> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    for e in v.iter_errors(&doc) {
        // The offending entry is the first segment of the instance path.
        let key = e
            .instance_path()
            .to_string()
            .trim_start_matches('/')
            .split('/')
            .next()
            .unwrap_or_default()
            .to_string();
        if e.to_string().contains("\"id\" is a required property") {
            missing_id.push(key);
        } else {
            other.push(format!("{key}: {e}"));
        }
    }
    missing_id.sort();

    assert!(other.is_empty(), "registry.toml schema errors: {other:#?}");
    assert_eq!(
        missing_id, ENTRIES_STILL_WITHOUT_AN_ID,
        "the set of entries without an `id` changed; update \
         ENTRIES_STILL_WITHOUT_AN_ID (shrinking it is the point)"
    );
}

fn wasm_base() -> Value {
    json!({
        "backend": { "id": "app.test.y", "source": "github.com/x/y", "name": "Y",
                      "version": "1.0.0", "kind": "wasm", "entrypoint": "y.wasm",
                      "contract": "v1", "license": "Apache-2.0",
                      "description": "Y backend." },
        "assets": { "wasm": "y.wasm" }
    })
}

/// A model entry that declares `role`, the field v2 introduced.
fn post_processor_model() -> Value {
    json!({
        "name": "cleanup", "role": "post_processor", "primary_language": "en",
        "supported_languages": ["en"], "supported_devices": ["cpu"]
    })
}

/// The schema is contract-aware the same way the parser is: `role` is
/// accepted under `contract = "v2"` and refused under `"v1"`, and the
/// refusal names the field so an editor points at the right line. This is
/// the schema half of `Manifest::parse`'s `FieldRequiresContract`.
#[test]
fn the_schema_gates_role_on_contract_v2() {
    let v = backend_validator();

    let mut v2 = wasm_base();
    v2["backend"]["contract"] = json!("v2");
    v2["models"] = json!([post_processor_model()]);
    let errors: Vec<String> = v.iter_errors(&v2).map(|e| e.to_string()).collect();
    assert!(
        errors.is_empty(),
        "role under v2 must validate: {errors:#?}"
    );

    let mut v1 = wasm_base();
    v1["models"] = json!([post_processor_model()]);
    let at: Vec<String> = v
        .iter_errors(&v1)
        .map(|e| e.instance_path().to_string())
        .collect();
    assert!(!at.is_empty(), "role under v1 must be refused");
    assert!(
        at.iter().any(|path| path.ends_with("/role")),
        "the refusal should point at the field: {at:#?}"
    );

    // A v1 manifest that stays within v1 is untouched by the rule.
    let mut plain = wasm_base();
    plain["models"] = json!([{
        "name": "m1", "primary_language": "en",
        "supported_languages": ["en"], "supported_devices": ["cpu"]
    }]);
    assert!(
        v.is_valid(&plain),
        "a v1 manifest without v2 fields stays valid"
    );
}

/// Every generation the parser knows is one the schema offers, and nothing
/// else: the `contract` enum is the closed set that gates old daemons, so it
/// must not drift from `Contract::ALL`.
#[test]
fn the_schema_offers_exactly_the_known_contracts() {
    use super_stt_registry_types::manifest::Contract;
    use super_stt_registry_types::product::Generation;
    let schema = super_stt_registry_types::schema::backend_schema();
    // Documented variants come out of schemars as a `oneOf` of `const`s.
    let offered: Vec<Value> = schema["definitions"]["Contract"]["oneOf"]
        .as_array()
        .expect("Contract definition must be a oneOf of consts")
        .iter()
        .map(|v| v["const"].clone())
        .collect();
    let known: Vec<Value> = Contract::ALL.iter().map(|c| json!(c.to_string())).collect();
    assert_eq!(offered, known);
}

/// The contract rule references field names as string literals; a serde
/// rename of `role` would make the rule disallow a key that no longer exists
/// while the real one slips through.
#[test]
fn contract_rule_field_names_exist() {
    use super_stt_registry_types::manifest::CONTRACT_FIELDS;
    let schema = super_stt_registry_types::schema::backend_schema();
    let defs = schema["definitions"].as_object().expect("definitions");
    for field in CONTRACT_FIELDS {
        // A row naming a table the schema builder cannot map would generate a
        // rule matching nothing — silently un-gating the field. That is a
        // failure to report, not a reason to abort the run.
        let Some(def) = field.schema_definition() else {
            panic!(
                "{}: no schema definition mapped for table `{}`",
                field.path(),
                field.table
            );
        };
        assert!(
            defs.contains_key(def),
            "{}: schema has no `{def}` definition",
            field.path()
        );
        assert!(
            defs[def]["properties"]
                .as_object()
                .is_some_and(|p| p.contains_key(field.key)),
            "{} is not a property of {def}",
            field.path()
        );
    }
}
