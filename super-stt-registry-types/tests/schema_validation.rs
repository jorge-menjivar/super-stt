// SPDX-License-Identifier: GPL-3.0-only
//! The generated schema must accept every in-repo manifest and reject the
//! contract violations the cross-field conditionals exist for.
#![cfg(feature = "schema")]

use serde_json::{Value, json};

fn backend_validator() -> jsonschema::Validator {
    jsonschema::validator_for(&super_stt_registry_types::schema::backend_schema())
        .expect("backend schema compiles")
}

fn toml_to_json(text: &str) -> Value {
    toml::from_str(text).expect("valid TOML")
}

#[test]
fn accepts_every_in_repo_backend_toml() {
    let v = backend_validator();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    // Backends are migrating to their own repos, so the in-repo set shrinks
    // over time (and `backends/` may eventually disappear) — validate whatever
    // manifests remain rather than asserting a fixed count. The schema's
    // acceptance of a valid manifest is also covered by the inline cases in
    // `allows_documented_optionals` / `rejects_contract_violations`.
    let Ok(dir) = std::fs::read_dir(root.join("backends")) else {
        return;
    };
    for entry in dir.flatten() {
        let path = entry.path().join("backend.toml");
        if !path.exists() {
            continue;
        }
        let doc = toml_to_json(&std::fs::read_to_string(&path).unwrap());
        let errors: Vec<String> = v.iter_errors(&doc).map(|e| format!("{e}")).collect();
        assert!(
            errors.is_empty(),
            "{} schema errors: {errors:#?}",
            path.display()
        );
    }
}

#[test]
fn accepts_registry_toml() {
    let v = jsonschema::validator_for(&super_stt_registry_types::schema::registry_schema())
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

fn wasm_base() -> Value {
    json!({
        "backend": { "source": "github.com/x/y", "name": "Y", "version": "1.0.0",
                      "kind": "wasm", "entrypoint": "y.wasm", "contract": "v1",
                      "license": "Apache-2.0" },
        "assets": { "wasm": "y.wasm" }
    })
}

fn sub_base() -> Value {
    json!({
        "backend": { "source": "github.com/x/y", "name": "Y", "version": "1.0.0",
                      "kind": "subprocess", "entrypoint": "y", "contract": "v1",
                      "license": "Apache-2.0" },
        "assets": { "subprocess": [
            { "file": "y.tgz", "target": "x86_64-unknown-linux-gnu", "accel": "cpu" }
        ] }
    })
}

#[test]
fn rejects_contract_violations() {
    let v = backend_validator();
    // The rejection cases are one mutation away from these bases; if a base
    // were itself invalid, every rejection below would pass vacuously.
    assert!(v.is_valid(&wasm_base()), "wasm_base must be valid");
    assert!(v.is_valid(&sub_base()), "sub_base must be valid");
    let cases: Vec<(&str, Value)> = vec![
        ("wasm with assets table but no wasm key", {
            let mut d = wasm_base();
            d["assets"] = json!({});
            d
        }),
        ("subprocess with empty asset list", {
            let mut d = sub_base();
            d["assets"]["subprocess"] = json!([]);
            d
        }),
        ("cuda asset missing cuda_major", {
            let mut d = sub_base();
            d["assets"]["subprocess"] = json!([
                { "file": "y.tgz", "target": "t", "accel": "cuda" }
            ]);
            d
        }),
        ("cpu asset with cuda fields", {
            let mut d = sub_base();
            d["assets"]["subprocess"] = json!([
                { "file": "y.tgz", "target": "t", "accel": "cpu", "cuda_major": 12 }
            ]);
            d
        }),
        ("cudnn on a cpu asset", {
            let mut d = sub_base();
            d["assets"]["subprocess"] = json!([
                { "file": "y.tgz", "target": "t", "accel": "cpu", "cudnn": true }
            ]);
            d
        }),
        ("unknown provider", {
            let mut d = wasm_base();
            d["models"] = json!([{ "name": "m", "provider": "anthropic",
                "primary_language": "en", "supported_languages": ["en"],
                "supported_devices": ["none"] }]);
            d
        }),
        ("huggingface files without repo/files", {
            let mut d = wasm_base();
            d["models"] = json!([{ "name": "m", "provider": "openai",
                "primary_language": "en", "supported_languages": ["en"],
                "supported_devices": ["none"],
                "files": [{ "source": "huggingface", "dest": "models/m" }] }]);
            d
        }),
        ("unknown top-level table", {
            let mut d = wasm_base();
            d["frobnicate"] = json!(true);
            d
        }),
        ("model with empty supported_devices", {
            let mut d = wasm_base();
            d["models"] = json!([{ "name": "m", "provider": "openai",
                "primary_language": "en", "supported_languages": ["en"],
                "supported_devices": [] }]);
            d
        }),
        ("assets present but license missing", {
            let mut d = wasm_base();
            d["backend"].as_object_mut().unwrap().remove("license");
            d
        }),
        ("unrecognized license value", {
            let mut d = wasm_base();
            d["backend"]["license"] = json!("Definitely-Not-A-License");
            d
        }),
    ];
    for (label, doc) in cases {
        assert!(!v.is_valid(&doc), "{label}: should have failed validation");
    }
}

/// `close_objects` only walks root + definitions; if a future type change
/// produces inline object schemas elsewhere, strictness would silently be
/// lost. Walk the whole output and fail loudly instead.
#[test]
fn every_data_object_is_closed() {
    fn walk(v: &Value, path: &str, errors: &mut Vec<String>) {
        match v {
            Value::Object(obj) => {
                if obj.contains_key("properties")
                    && obj.get("additionalProperties") != Some(&Value::Bool(false))
                {
                    errors.push(path.to_string());
                }
                for (k, child) in obj {
                    // Conditional branches intentionally stay open: a closed
                    // `then` listing only `assets` would reject everything.
                    if matches!(k.as_str(), "if" | "then" | "else") {
                        continue;
                    }
                    walk(child, &format!("{path}/{k}"), errors);
                }
            }
            Value::Array(items) => {
                for (i, child) in items.iter().enumerate() {
                    walk(child, &format!("{path}/{i}"), errors);
                }
            }
            _ => {}
        }
    }
    for (name, schema) in [
        (
            "backend",
            super_stt_registry_types::schema::backend_schema(),
        ),
        (
            "registry",
            super_stt_registry_types::schema::registry_schema(),
        ),
    ] {
        let mut errors = Vec::new();
        walk(&schema, name, &mut errors);
        assert!(errors.is_empty(), "open object schemas: {errors:#?}");
    }
}

/// The injected conditionals reference property names as string literals; a
/// serde rename would make an `if` never fire, silently dropping the rule.
#[test]
fn conditional_property_names_exist() {
    let schema = super_stt_registry_types::schema::backend_schema();
    let root_props = schema["properties"].as_object().expect("root properties");
    for key in ["backend", "assets"] {
        assert!(root_props.contains_key(key), "root missing `{key}`");
    }
    let defs = schema["definitions"].as_object().expect("definitions");
    let asset_props = defs["SubprocessAsset"]["properties"]
        .as_object()
        .expect("SubprocessAsset properties");
    for key in ["accel", "cuda_major", "cuda_sm", "cudnn"] {
        assert!(
            asset_props.contains_key(key),
            "SubprocessAsset missing `{key}`"
        );
    }
    let files_props = defs["FilesSpec"]["properties"]
        .as_object()
        .expect("FilesSpec properties");
    for key in ["source", "url", "repo", "files", "dest"] {
        assert!(files_props.contains_key(key), "FilesSpec missing `{key}`");
    }
    let backend_props = defs["BackendMeta"]["properties"]
        .as_object()
        .expect("BackendMeta properties");
    for key in ["kind", "license"] {
        assert!(backend_props.contains_key(key), "BackendMeta missing `{key}`");
    }
    // The license value-set is injected as an enum; a rename or a dropped
    // injection would silently stop constraining it.
    let license_enum = backend_props["license"]["enum"]
        .as_array()
        .expect("license property must carry an injected enum");
    assert!(
        license_enum.iter().any(|v| v == "Apache-2.0")
            && license_enum.iter().any(|v| v == "other"),
        "license enum must include known SPDX ids and `other`"
    );
    let assets_props = defs["Assets"]["properties"]
        .as_object()
        .expect("Assets properties");
    for key in ["wasm", "subprocess"] {
        assert!(assets_props.contains_key(key), "Assets missing `{key}`");
    }
    let model_entry_props = defs["ModelEntry"]["properties"]
        .as_object()
        .expect("ModelEntry properties");
    assert!(
        model_entry_props.contains_key("supported_devices"),
        "ModelEntry missing `supported_devices`"
    );
}

#[test]
fn allows_documented_optionals() {
    let v = backend_validator();
    // No [assets] at all — legitimate for locally installed backends, which may
    // also omit the license (only publication requires it).
    let mut local = wasm_base();
    {
        let obj = local.as_object_mut().unwrap();
        obj.remove("assets");
        obj["backend"].as_object_mut().unwrap().remove("license");
    }
    assert!(
        v.is_valid(&local),
        "manifest without [assets] or license must validate"
    );
    // The explicit `other` escape is an accepted license value.
    let mut other = wasm_base();
    other["backend"]["license"] = json!("other");
    assert!(v.is_valid(&other), "license = \"other\" must validate");
    // cuda_major without cuda_sm — the wildcard-SM build.
    let mut wildcard = sub_base();
    wildcard["assets"]["subprocess"] = json!([
        { "file": "y.tgz", "target": "t", "accel": "cuda", "cuda_major": 13 }
    ]);
    assert!(v.is_valid(&wildcard), "wildcard cuda_sm must validate");
}
