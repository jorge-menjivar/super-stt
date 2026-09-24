// SPDX-License-Identifier: GPL-3.0-only
//! `super-stt-indexer`: builds Super STT's backend registry index. The
//! indexer is `super_engine_indexer`'s; what is here is what makes it Super
//! STT's.

use super_engine_indexer::Indexer;
use super_stt_registry_types::Stt;

/// Entries that predate the `id` requirement, and the migration backlog.
///
/// `registry.schema.json` requires an `id` of *every* entry, so an editor
/// already flags these six. They are tolerated here so the scheduled index
/// build keeps publishing a catalog while they are fixed one at a time —
/// removing a name without fixing its backend would take every user's Browse
/// listing down with it.
///
/// Fixing one is not a `registry.toml` edit. An entry that declares an `id`
/// pins the release to it, so the order is: add `[backend].id` to that
/// backend's own `backend.toml`, cut a release, *then* add the same id here
/// and delete the name below. Adding the id here first earns an `IdMismatch`.
const GRANDFATHERED: &[&str] = &[
    "deepgram",
    "mistral",
    "openai",
    "qwen3_asr",
    "voxtral",
    "whisper",
];

const INDEXER: Indexer = Indexer {
    name: "super-stt-indexer",
    user_agent: Stt::USER_AGENT,
    grandfathered: GRANDFATHERED,
};

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    super_engine_indexer::main::<Stt>(&INDEXER).await
}

#[cfg(test)]
mod tests {
    use super::GRANDFATHERED;
    use super_engine_indexer::registry_toml::Registry;

    /// The shipped file must keep parsing as the `id` requirement lands.
    #[test]
    fn the_in_repo_registry_parses() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap();
        let text = std::fs::read_to_string(root.join("registry/registry.toml")).unwrap();
        Registry::parse(&text, GRANDFATHERED).expect("registry.toml parses");
    }

    /// The entries still being migrated keep publishing without an `id`.
    #[test]
    fn accepts_a_grandfathered_entry_without_an_id() {
        let text = "[voxtral]\n    repo = \"github.com/jorge-menjivar/super-stt-voxtral\"\n    forge = \"github\"\n";
        Registry::parse(text, GRANDFATHERED).expect("existing entries are exempt");
    }
}
