// SPDX-License-Identifier: GPL-3.0-only
//! Committed schema files must match what the types generate.
#![cfg(feature = "schema")]

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn committed_schemas_are_current() {
    let cases = [
        (
            "schemas/backend.schema.json",
            super_stt_registry_types::schema::backend_schema_pretty(),
        ),
        (
            "schemas/registry.schema.json",
            super_stt_registry_types::schema::registry_schema_pretty(),
        ),
    ];
    for (rel, generated) in cases {
        let committed = std::fs::read_to_string(repo_root().join(rel))
            .unwrap_or_else(|e| panic!("read {rel}: {e}"));
        if committed != generated {
            let first_diff = committed
                .lines()
                .zip(generated.lines())
                .position(|(c, g)| c != g)
                .map_or_else(
                    || "(length differs)".to_string(),
                    |n| format!("line {}", n + 1),
                );
            panic!(
                "{rel} is out of date (first difference at {first_diff}) — \
                 run `just gen-schemas` and commit the result"
            );
        }
    }
}
