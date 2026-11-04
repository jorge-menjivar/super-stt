// SPDX-License-Identifier: GPL-3.0-only
//! `super-tts-indexer`: builds Super TTS's backend registry index. The
//! indexer is `super_engine_indexer`'s; what is here is what makes it Super
//! TTS's.

use super_engine_indexer::Indexer;
use super_tts_registry_types::Tts;

const INDEXER: Indexer = Indexer {
    name: "super-tts-indexer",
    user_agent: Tts::USER_AGENT,
    // Every entry must declare an `id`. The exemptions were the ASR catalog
    // inherited from Super STT, and `registry/registry.toml` no longer lists
    // them, so nothing predates the requirement now.
    grandfathered: &[],
};

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    super_engine_indexer::main::<Tts>(&INDEXER).await
}

#[cfg(test)]
mod tests {
    use super::INDEXER;
    use super_engine_indexer::registry_toml::Registry;

    /// The shipped file must keep parsing, every entry with its `id`.
    #[test]
    fn the_in_repo_registry_parses() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap();
        let text = std::fs::read_to_string(root.join("registry/registry.toml")).unwrap();
        Registry::parse(&text, INDEXER.grandfathered).expect("registry.toml parses");
    }
}
