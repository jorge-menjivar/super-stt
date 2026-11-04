// SPDX-License-Identifier: GPL-3.0-only
//! The generated schema must accept every in-repo manifest and the repo's
//! `registry.toml`. The rules every product shares are tested in
//! `super-engine-spec`.
#![cfg(feature = "schema")]

use serde_json::Value;

fn backend_validator() -> jsonschema::Validator {
    jsonschema::validator_for(&super_tts_registry_types::schema::backend_schema())
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
        .join("super-tts-daemon/tests/fixtures");
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
        .join("super-tts-daemon/tests/fixtures");
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

#[test]
fn accepts_registry_toml() {
    let v = jsonschema::validator_for(&super_tts_registry_types::schema::registry_schema())
        .expect("registry schema compiles");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let doc = toml_to_json(&std::fs::read_to_string(root.join("registry/registry.toml")).unwrap());
    let errors: Vec<String> = v.iter_errors(&doc).map(|e| format!("{e}")).collect();
    assert!(
        errors.is_empty(),
        "registry.toml schema errors: {errors:#?}"
    );
}

/// `[[models]].prepare_files` is a v1 field, so the schema takes it on a v1
/// manifest. The schema half of `prepare_files_parse_under_v1`.
#[test]
fn the_schema_accepts_prepare_files_under_v1() {
    let doc = toml_to_json(
        r#"
[backend]
source = "github.com/x/y"
name = "Y"
version = "1.0.0"
kind = "subprocess"
entrypoint = "y"
product = "super-tts"
contract = "v1"
description = "Test backend."

[[models]]
name = "m"
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu"]
prepare_files = ["m/config.json"]
files = [{ url = "https://h/config.json", destination = "m/config.json" }]
"#,
    );
    let errors: Vec<String> = backend_validator()
        .iter_errors(&doc)
        .map(|e| format!("{} at {}", e, e.instance_path()))
        .collect();
    assert!(
        errors.is_empty(),
        "prepare_files under v1 must validate: {errors:#?}"
    );
}
