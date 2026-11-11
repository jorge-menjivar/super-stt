// SPDX-License-Identifier: GPL-3.0-only
//! `super-voice-indexer`: builds Super Voice's backend registry index. The
//! indexer is `super_engine_indexer`'s; what is here is what makes it Super
//! Voice's.
//!
//! Every registry entry must declare an `id`, and the indexer refuses one
//! without. The entries that predated the requirement were exempt until each
//! backend's own release declared one; Deepgram was the last, and the
//! exemption is gone.

use super_engine_indexer::Indexer;
use super_voice_registry_types::Voice;

const INDEXER: Indexer = Indexer {
    name: "super-voice-indexer",
    user_agent: Voice::USER_AGENT,
};

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    super_engine_indexer::main::<Voice>(&INDEXER).await
}

#[cfg(test)]
mod tests {
    use super_engine_indexer::registry_toml::Registry;

    /// The shipped file must keep parsing as the `id` requirement lands.
    #[test]
    fn the_in_repo_registry_parses() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap();
        let text = std::fs::read_to_string(root.join("registry/registry.toml")).unwrap();
        Registry::parse(&text).expect("registry.toml parses");
    }

    /// An entry without an `id` is refused, the last formerly exempt name
    /// included.
    #[test]
    fn refuses_an_entry_without_an_id() {
        let text = "[deepgram]\n    repo = \"github.com/jorge-menjivar/super-stt-deepgram\"\n    forge = \"github\"\n";
        let parsed = Registry::parse(text);
        assert!(parsed.is_err(), "an entry without an id must be refused");
    }
}
