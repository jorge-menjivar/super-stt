// SPDX-License-Identifier: GPL-3.0-only

//! Transcription-language UI state, extracted from `AppModel` following the
//! `RegistryState` template (App Tier 3 #15).

use crate::state::LanguageResolution;

/// The resolution blocks the daemon has answered for, keyed by
/// `(source, model)` — the blocks behind
/// `GET /pipeline/{stage}/model/{model}/language`.
///
/// A single slot held one block while only the transcription card had a
/// language control. Both pipeline stages have one now, and a stage-1 fetch
/// would overwrite the block stage 2 is rendering (and the other way round),
/// so answers are keyed by the pair they describe — the same shape, and for
/// the same reason, as [`crate::state::device_offers::DeviceOffers`].
///
/// A miss means "not asked yet, or the answer hasn't landed", which is what
/// makes a stale answer harmless: a lookup for a pair simply misses until its
/// own answer arrives.
#[derive(Debug, Clone, Default)]
pub struct ModelLanguages {
    blocks: Vec<((u32, String, String), LanguageResolution)>,
    /// The tags each answered-for model can be pinned to — what
    /// `GET /pipeline/{stage}/model/{model}/language/list` returns.
    ///
    /// Held beside the blocks rather than inside them because the two arrive
    /// from different requests and have different lifetimes: the block changes
    /// every time the user picks a language, the offer never changes for a
    /// given model. A single record would make every pick re-fetch a list that
    /// cannot have moved.
    offers: Vec<((u32, String, String), Vec<String>)>,
}

impl ModelLanguages {
    /// The block for `(stage, source, model)`, or `None` if the daemon hasn't
    /// answered for it.
    #[must_use]
    pub fn get(&self, stage: u32, source: &str, model: &str) -> Option<&LanguageResolution> {
        self.blocks
            .iter()
            .find(|((st, s, m), _)| *st == stage && s == source && m == model)
            .map(|(_, block)| block)
    }

    /// Record an answer, replacing any it supersedes.
    pub fn record(&mut self, stage: u32, source: String, model: String, block: LanguageResolution) {
        let key = (stage, source, model);
        if let Some(existing) = self.blocks.iter_mut().find(|(k, _)| *k == key) {
            existing.1 = block;
            return;
        }
        self.blocks.push((key, block));
    }

    /// Every pair answered for, so a global language change can refresh all of
    /// them: a per-model block that follows the global value goes stale the
    /// moment that value changes, whichever card is showing it.
    pub fn pairs(&self) -> impl Iterator<Item = (u32, String, String)> + '_ {
        self.blocks.iter().map(|(key, _)| key.clone())
    }

    /// The tags `(stage, source, model)` can be pinned to, or `None` if the
    /// daemon has not answered for it.
    ///
    /// A miss and an empty answer are different, and a picker reads them
    /// differently: not asked yet is a control still loading, while an empty
    /// offer is a monolingual model with nothing to choose.
    #[must_use]
    pub fn offered(&self, stage: u32, source: &str, model: &str) -> Option<&[String]> {
        self.offers
            .iter()
            .find(|((st, s, m), _)| *st == stage && s == source && m == model)
            .map(|(_, tags)| tags.as_slice())
    }

    /// Record what a model can be pinned to.
    pub fn record_offer(&mut self, stage: u32, source: String, model: String, tags: Vec<String>) {
        let key = (stage, source, model);
        if let Some(existing) = self.offers.iter_mut().find(|(k, _)| *k == key) {
            existing.1 = tags;
            return;
        }
        self.offers.push((key, tags));
    }
}

/// The global Primary Language plus the per-model override picker state.
#[derive(Debug, Clone, Default)]
pub struct LanguageState {
    /// Global Primary Language from the daemon (`None` = unset). Display-only cache.
    pub primary_language: Option<String>,
    /// Per-model resolution blocks, one per `(source, model)` asked about.
    pub model_languages: ModelLanguages,
    /// The `(source, model)` pair the open per-model language sheet configures.
    /// `None` when the sheet is in global mode.
    pub language_picker_target: Option<(u32, String, String)>,
    /// Live query text for the language search sheet.
    pub language_picker_query: String,
}

#[cfg(test)]
mod tests {
    use super::ModelLanguages;
    use crate::state::LanguageResolution;

    fn block(primary: &str) -> LanguageResolution {
        LanguageResolution {
            effective: Some(primary.to_string()),
            source: "default".to_string(),
            primary: primary.to_string(),
        }
    }

    /// The regression the store exists for: two stages each showing a language
    /// control must not overwrite each other's block. A single slot meant the
    /// second fetch to land won and the other card fell back to a neutral
    /// label.
    #[test]
    fn two_models_are_held_at_once() {
        let mut langs = ModelLanguages::default();
        langs.record(1, "src/a".into(), "whisper".into(), block("es"));
        langs.record(2, "src/b".into(), "s1-mini".into(), block("en"));

        assert_eq!(langs.get(1, "src/a", "whisper").unwrap().primary, "es");
        assert_eq!(langs.get(2, "src/b", "s1-mini").unwrap().primary, "en");
    }

    /// The same `(source, model)` in two stages is two entries, not one.
    ///
    /// A backend serving a model both stages can run is the case that needs it:
    /// keyed on the pair alone, staging it in one card would answer for the
    /// other, and a language change in stage 1 would silently relabel stage 2.
    #[test]
    fn one_pair_in_two_stages_is_two_entries() {
        let mut langs = ModelLanguages::default();
        langs.record(1, "src/a".into(), "shared".into(), block("es"));
        langs.record(2, "src/a".into(), "shared".into(), block("en"));

        assert_eq!(langs.get(1, "src/a", "shared").unwrap().primary, "es");
        assert_eq!(langs.get(2, "src/a", "shared").unwrap().primary, "en");
        assert_eq!(langs.pairs().count(), 2);
    }

    /// A second answer for the same pair replaces the first rather than
    /// stacking, or a language change would never be reflected.
    #[test]
    fn a_new_answer_replaces_the_pairs_old_one() {
        let mut langs = ModelLanguages::default();
        langs.record(1, "src/a".into(), "whisper".into(), block("es"));
        langs.record(1, "src/a".into(), "whisper".into(), block("de"));

        assert_eq!(langs.get(1, "src/a", "whisper").unwrap().primary, "de");
        assert_eq!(langs.pairs().count(), 1);
    }

    /// A pair never asked about misses, which is what the callers render as
    /// "not answered yet" rather than as an absent language.
    #[test]
    fn an_unasked_pair_misses() {
        let langs = ModelLanguages::default();
        assert!(langs.get(1, "src/a", "whisper").is_none());
    }

    /// An offer of nothing is not the same as no offer, and the picker draws
    /// them differently: a monolingual model has nothing to choose, while an
    /// unanswered one is still loading.
    #[test]
    fn an_empty_offer_is_not_a_missing_one() {
        let mut langs = ModelLanguages::default();
        assert!(langs.offered(1, "src/a", "mono").is_none());

        langs.record_offer(1, "src/a".into(), "mono".into(), Vec::new());
        assert_eq!(langs.offered(1, "src/a", "mono"), Some(&[][..]));
    }

    /// Offers are keyed like blocks, so two stages showing the same model do
    /// not answer for each other.
    #[test]
    fn offers_are_keyed_by_stage_too() {
        let mut langs = ModelLanguages::default();
        langs.record_offer(1, "src/a".into(), "shared".into(), vec!["auto".into()]);
        langs.record_offer(2, "src/a".into(), "shared".into(), Vec::new());

        assert_eq!(
            langs.offered(1, "src/a", "shared").map(<[String]>::len),
            Some(1)
        );
        assert_eq!(
            langs.offered(2, "src/a", "shared").map(<[String]>::len),
            Some(0)
        );
    }
}
