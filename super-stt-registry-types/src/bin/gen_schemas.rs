// SPDX-License-Identifier: GPL-3.0-only
//! Writes the generated JSON Schemas to a gitignored `target/schemas/` for
//! local inspection. The schemas are not committed: CI regenerates them and
//! publishes to the gh-pages branch alongside `index.json`.

use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate lives one level under the repo root");
    let targets = [
        (
            root.join("target/schemas/backend.schema.json"),
            super_stt_registry_types::schema::backend_schema_pretty(),
        ),
        (
            root.join("target/schemas/registry.schema.json"),
            super_stt_registry_types::schema::registry_schema_pretty(),
        ),
    ];
    for (path, text) in targets {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("create {}: {e}", parent.display()));
        }
        std::fs::write(&path, text).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        println!("wrote {}", path.display());
    }
}
