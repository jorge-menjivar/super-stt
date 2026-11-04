// SPDX-License-Identifier: GPL-3.0-only
//! The generated schema must accept every in-repo manifest, and must encode
//! Super STT's own contract table. The rules every product shares are tested
//! in `super-engine-spec`.
#![cfg(feature = "schema")]

use serde_json::{Value, json};

fn backend_validator() -> jsonschema::Validator {
    jsonschema::validator_for(&super_voice_registry_types::schema::backend_schema())
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
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let dirs = [
        root.join("super-voice-daemon/tests/fixtures"),
        root.join("super-tts-daemon/tests/fixtures"),
    ];
    let entries = dirs
        .iter()
        .flat_map(|dir| std::fs::read_dir(dir).expect("daemon test fixtures directory"));

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
        checked >= dirs.len(),
        "found {checked} backend manifests under {dirs:?}, fewer than one a daemon — this test \
         would pass while validating nothing"
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
        .join("super-voice-daemon/tests/fixtures");
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

/// The schema requires an `id` of every entry. Two predate the requirement
/// and cannot be fixed from this file: an entry that declares an `id` pins the
/// release to it, and neither of those two backends publishes a manifest
/// declaring one — so adding it here would turn a missing id into an
/// `IdMismatch` and drop the backend out of the catalog. Fixing one means
/// adding `[backend].id` to that backend's own repo, cutting a release, and
/// only then editing this file.
///
/// They are named here so the backlog is visible and shrinking: delete a name
/// when its entry gains an `id`. Anything *else* the schema objects to — a new
/// entry with no id, a malformed key — fails this test.
const ENTRIES_STILL_WITHOUT_AN_ID: &[&str] = &["deepgram", "voxtral"];

#[test]
fn accepts_registry_toml() {
    let v = jsonschema::validator_for(&super_voice_registry_types::schema::registry_schema())
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
                      "contract": "v1", "product": "super-voice", "license": "Apache-2.0",
                      "description": "Y backend." },
        "assets": { "wasm": "y.wasm" }
    })
}

/// A model entry of `role`.
fn model(role: &str) -> Value {
    json!({
        "name": "m1", "role": role, "primary_language": "en",
        "supported_languages": ["en"], "supported_devices": ["cpu"]
    })
}

/// Every model declares its role, and each of the three validates. A model
/// without one is refused at the model, so an editor points at the right
/// line. The schema half of `Manifest::parse` requiring the key.
#[test]
fn the_schema_requires_each_model_to_declare_its_role() {
    let v = backend_validator();
    for role in ["transcription", "post_processor", "synthesis"] {
        let mut manifest = wasm_base();
        manifest["models"] = json!([model(role)]);
        let errors: Vec<String> = v.iter_errors(&manifest).map(|e| e.to_string()).collect();
        assert!(
            errors.is_empty(),
            "role = {role} must validate: {errors:#?}"
        );
    }

    let mut without = wasm_base();
    let mut m = model("transcription");
    m.as_object_mut().unwrap().remove("role");
    without["models"] = json!([m]);
    let at: Vec<String> = v
        .iter_errors(&without)
        .map(|e| e.instance_path().to_string())
        .collect();
    assert!(
        at.iter().any(|path| path.ends_with("/models/0")),
        "a model without a role must be refused at the model: {at:#?}"
    );
}

/// `[backend].product` takes `super-voice` and nothing else, and v1 requires
/// it. The schema half of `Manifest::parse`'s `OtherProduct` and
/// `FieldRequiredByContract`.
#[test]
fn the_schema_takes_only_super_voice_and_requires_it() {
    let v = backend_validator();
    assert!(v.is_valid(&wasm_base()), "a Super Voice manifest validates");

    for other in ["super-stt", "super-tts"] {
        let mut manifest = wasm_base();
        manifest["backend"]["product"] = json!(other);
        assert!(!v.is_valid(&manifest), "a {other} manifest is refused");
    }

    let mut without = wasm_base();
    without["backend"]
        .as_object_mut()
        .unwrap()
        .remove("product");
    assert!(!v.is_valid(&without), "v1 requires the product");
}

/// Every generation the parser knows is one the schema offers, and nothing
/// else: the `contract` enum is the closed set that gates old daemons, so it
/// must not drift from `Contract::ALL`.
#[test]
fn the_schema_offers_exactly_the_known_contracts() {
    use super_voice_registry_types::manifest::Contract;
    use super_voice_registry_types::product::Generation;
    let schema = super_voice_registry_types::schema::backend_schema();
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
    use super_voice_registry_types::manifest::CONTRACT_FIELDS;
    let schema = super_voice_registry_types::schema::backend_schema();
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

/// `[[models]].prepare_files` is a v1 field. The schema half of
/// `Manifest::parse` taking it.
#[test]
fn the_schema_accepts_prepare_files_under_v1() {
    let v = backend_validator();
    let mut m = model("transcription");
    m["prepare_files"] = json!(["m/config.json"]);
    m["files"] = json!([{ "url": "https://h/config.json", "destination": "m/config.json" }]);
    let mut manifest = wasm_base();
    manifest["models"] = json!([m]);
    let errors: Vec<String> = v.iter_errors(&manifest).map(|e| e.to_string()).collect();
    assert!(
        errors.is_empty(),
        "prepare_files under v1 must validate: {errors:#?}"
    );
}
