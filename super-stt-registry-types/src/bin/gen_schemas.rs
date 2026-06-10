// SPDX-License-Identifier: GPL-3.0-only
//! Writes the generated JSON Schemas into the repo. Run via
//! `just gen-schemas`; CI fails if the committed files are stale.

use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate lives one level under the repo root");
    let targets = [
        (
            root.join("docs/protocol/backend/backend.schema.json"),
            super_stt_registry_types::schema::backend_schema_pretty(),
        ),
        (
            root.join("registry/registry.schema.json"),
            super_stt_registry_types::schema::registry_schema_pretty(),
        ),
    ];
    for (path, text) in targets {
        std::fs::write(&path, text).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        println!("wrote {}", path.display());
    }
}
