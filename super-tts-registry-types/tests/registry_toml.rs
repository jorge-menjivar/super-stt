// SPDX-License-Identifier: GPL-3.0-only
//! The repo's own `registry/registry.toml` parses into registry entries.

use super_tts_registry_types::entry::Entry;

#[test]
fn parses_in_repo_registry_toml() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let text = std::fs::read_to_string(root.join("registry/registry.toml")).unwrap();
    let map: std::collections::BTreeMap<String, Entry> = toml::from_str(&text).unwrap();
    // Deliberately not asserting non-empty: the catalog is empty until the
    // first TTS backend is published (the inherited entries were ASR
    // backends — see registry/registry.toml). What this test pins is that
    // whatever the file does contain parses and is well-formed.
    for (id, e) in &map {
        assert!(!e.repo.is_empty(), "entry {id} has empty repo");
    }
}
