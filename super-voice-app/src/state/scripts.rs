// SPDX-License-Identifier: GPL-3.0-only
//! Reading scripts for a cloned-voice reference clip.
//!
//! Recording a voice used to mean improvising: the user pressed Record with
//! nothing to say, said something forgettable, and then had to type it back
//! from memory into the transcript field. Both halves of that are avoidable —
//! the words can be chosen in advance, and a script that was read is a
//! transcript that is already written.
//!
//! What the scripts contain is not arbitrary. The published guidance for
//! in-context cloning models agrees on four things, and each shaped this table:
//!
//! * **Ten to thirty seconds of continuous speech.** Quality rises roughly
//!   linearly from three seconds to fifteen, then plateaus; much past that adds
//!   nothing and eventually hurts. Every script here is timed to land in that
//!   window at an unhurried pace, which `estimated_seconds` asserts in a test.
//! * **Continuous prose, not a word list.** Disconnected sentences leave long
//!   pauses between them, and a reference clip wants speech across most of its
//!   duration. That is why the phonetically-balanced sentence lists used in
//!   telephony testing (the Harvard sentences and their kin) are *not* here:
//!   balanced they may be, but they are read in ten separate clipped breaths.
//! * **Varied intonation.** A monotone reference produces a monotone clone, so
//!   the scripts carry questions, asides and changes of footing rather than a
//!   flat declarative paragraph.
//! * **Phonetic coverage**, which is what makes a sound the model never heard
//!   less of a guess. [`EVERY_SOUND`] covers the full American English phoneme
//!   inventory — verified by phonemizing it with espeak-ng and checking the
//!   result against the inventory, not by eye.
//!
//! The texts are original to this project or public domain, so the catalog can
//! ship under the same license as the rest of the app.

/// One script: what to read, and what it is for.
///
/// `'static` throughout. The catalog is a table compiled into the binary, so a
/// chosen script is a `Copy` handle that costs nothing to keep in page state
/// and cannot go stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Script {
    /// Stable identifier, used to carry a choice through a message.
    pub id: &'static str,
    /// Short name, as it appears in the picker.
    pub title: &'static str,
    /// BCP-47 tag of the language the text is written in.
    pub language: &'static str,
    /// One line on what this script is good for.
    pub blurb: &'static str,
    /// The words to read, and — once read — the clip's transcript.
    pub text: &'static str,
}

/// Read-aloud pace for a space-delimited language: ~150 words per minute, which
/// is an unhurried reading voice rather than a hurried one.
const WORDS_PER_SECOND: f32 = 2.5;

/// Read-aloud pace for Japanese and Chinese, which have no word spaces. Counted
/// per character, since a kana is about a mora and a hanzi about a syllable.
const CJK_CHARS_PER_SECOND: f32 = 5.5;

impl Script {
    /// Roughly how long this script takes to read aloud.
    ///
    /// Derived from the text rather than stored beside it: a hand-written
    /// duration is one edit away from being a lie, and this number is shown to
    /// the user and compared against the model's reference-audio budget.
    #[must_use]
    pub fn estimated_seconds(&self) -> f32 {
        let mut cjk = 0_usize;
        let mut spaced = String::with_capacity(self.text.len());
        for ch in self.text.chars() {
            if is_cjk(ch) {
                cjk += 1;
            } else {
                spaced.push(ch);
            }
        }
        // Punctuation left behind by a CJK text ("、" and friends) is not a
        // word; only tokens with a letter in them are counted.
        let words = spaced
            .split_whitespace()
            .filter(|w| w.chars().any(char::is_alphabetic))
            .count();
        #[allow(clippy::cast_precision_loss)]
        let (words, cjk) = (words as f32, cjk as f32);
        words / WORDS_PER_SECOND + cjk / CJK_CHARS_PER_SECOND
    }
}

impl Script {
    /// Whether this script can be read inside a model's reference-audio
    /// budget.
    ///
    /// Overrunning is not merely wasted breath. The daemon trims a stored clip
    /// to `clone_ref_seconds` but sends the transcript whole, so a script read
    /// past the budget hands an in-context model audio that stops mid-sentence
    /// described by words that carry on — and that pair is what the model
    /// imitates. A model with no declared budget takes whatever it is given.
    #[must_use]
    pub fn fits(&self, budget: Option<f32>) -> bool {
        budget.is_none_or(|budget| self.estimated_seconds() <= budget)
    }
}

/// Whether a character is Han, hiragana or katakana — the scripts that are
/// written without spaces, and so cannot be timed by counting words.
fn is_cjk(ch: char) -> bool {
    matches!(ch,
        '\u{3040}'..='\u{30ff}'      // hiragana + katakana
        | '\u{3400}'..='\u{4dbf}'    // CJK unified ideographs extension A
        | '\u{4e00}'..='\u{9fff}'    // CJK unified ideographs
        | '\u{f900}'..='\u{faff}'    // CJK compatibility ideographs
    )
}

/// A normal conversation, and the default for English.
///
/// The all-round reference: continuous, unhurried, with a question and an aside
/// so the model hears more than one intonation contour.
const EVERYDAY: Script = Script {
    id: "en-everyday",
    title: "Everyday conversation",
    language: "en",
    blurb: "An ordinary, unhurried exchange. The best all-round reference, and the one to use \
            if you are unsure.",
    text: "Thanks for waiting. I just got back from the shop, and the rain finally stopped. \
           Did you want the blue jacket or the green one? Either would look good, honestly.",
};

/// Every phoneme in American English, in continuous prose.
///
/// Coverage is checked, not assumed: phonemizing this text with
/// `espeak-ng -v en-us --ipa` yields all twenty-four consonants and every
/// vowel and diphthong in the General American inventory, including the four
/// that ordinary prose most often misses — /ʒ/ (*measure*, *pleasure*), /θ/ and
/// /ð/ (*thought*, *the*), and /ɔɪ/ (*voice*, *joined*).
const EVERY_SOUND: Script = Script {
    id: "en-every-sound",
    title: "Every English sound",
    language: "en",
    blurb: "Touches every sound in English, so nothing in your voice has to be guessed at.",
    text: "The unusual measure of pleasure in her voice caught me off guard. She thought the \
           old bridge would be crowded, but the path along the shore was quiet enough to hear \
           the birds singing. We judged the walk a success, bought a cheap book at the corner \
           shop, and joined the others for a late lunch.",
};

/// Surprise, doubt, warmth and a request, in one breath each.
const EXPRESSIVE: Script = Script {
    id: "en-expressive",
    title: "Expressive range",
    language: "en",
    blurb: "Swings through surprise, doubt and warmth. Read it flat and the clone will sound \
            flat, so let it move.",
    text: "Wait, you're serious? That's wonderful news! Honestly, I didn't think it would \
           work, not after everything that went wrong last month. No, no, I'm glad. Really \
           glad. Look, whatever happens next, promise me you'll call me first.",
};

/// Aesop, in the standard phonetics-passage wording. Public domain, and the
/// passage the International Phonetic Association uses for language samples, so
/// many people have read it before.
const NORTH_WIND: Script = Script {
    id: "en-north-wind",
    title: "The North Wind and the Sun",
    language: "en",
    blurb: "The classic phonetics passage. Longer sentences and a steadier, more formal \
            delivery than the others.",
    text: "The North Wind and the Sun were disputing which was the stronger, when a traveller \
           came along wrapped in a warm cloak. They agreed that the one who first succeeded \
           in making the traveller take his cloak off should be considered stronger than the \
           other.",
};

/// Every script, in picker order: English first, then one conversation per
/// other language the shipped models speak.
///
/// A clone is best conditioned on speech in the language it will be asked to
/// speak, so the catalog covers the languages rather than leaving a non-English
/// speaker to improvise — which is the state this whole module exists to end.
pub const SCRIPTS: &[Script] = &[
    EVERYDAY,
    EVERY_SOUND,
    EXPRESSIVE,
    NORTH_WIND,
    Script {
        id: "es-everyday",
        title: "Everyday conversation",
        language: "es",
        blurb: "An ordinary, unhurried exchange in Spanish.",
        text: "Oye, ¿al final vienes el sábado? Si quieres paso por ti sobre las ocho y \
               llegamos juntos. Avísame mañana, que tengo que comprar las entradas. La \
               verdad, me haría mucha ilusión.",
    },
    Script {
        id: "fr-everyday",
        title: "Everyday conversation",
        language: "fr",
        blurb: "An ordinary, unhurried exchange in French.",
        text: "Écoute, je passe te prendre vers huit heures, d'accord ? On aura largement le \
               temps de dîner avant le film. Préviens-moi demain matin si tu changes d'avis. \
               Franchement, ça fait tellement longtemps.",
    },
    Script {
        id: "de-everyday",
        title: "Everyday conversation",
        language: "de",
        blurb: "An ordinary, unhurried exchange in German.",
        text: "Sag mal, kommst du am Samstag wirklich mit? Ich könnte dich gegen acht \
               abholen, dann sind wir rechtzeitig da. Gib mir bitte morgen Bescheid, ich \
               freue mich riesig.",
    },
    Script {
        id: "it-everyday",
        title: "Everyday conversation",
        language: "it",
        blurb: "An ordinary, unhurried exchange in Italian.",
        text: "Senti, alla fine vieni sabato? Se vuoi passo a prenderti verso le otto, così \
               arriviamo insieme. Fammi sapere domani, perché devo comprare i biglietti. Ci \
               sarà anche mia figlia.",
    },
    Script {
        id: "pt-br-everyday",
        title: "Everyday conversation",
        language: "pt-BR",
        blurb: "An ordinary, unhurried exchange in Brazilian Portuguese.",
        text: "Oi, você vai mesmo no sábado? Se quiser, eu passo aí por volta das oito e a \
               gente vai junto. Me avisa amanhã, porque preciso comprar os ingressos. Vai ser \
               ótimo te ver.",
    },
    Script {
        id: "hi-everyday",
        title: "Everyday conversation",
        language: "hi",
        blurb: "An ordinary, unhurried exchange in Hindi.",
        text: "सुनो, तुम शनिवार को सच में आ रहे हो? चाहो तो मैं आठ बजे तुम्हें लेने आ जाऊँगा, फिर हम साथ \
               चलते हैं। कल तक बता देना। सच कहूँ तो बहुत अच्छा लगेगा।",
    },
    Script {
        id: "ja-everyday",
        title: "Everyday conversation",
        language: "ja",
        blurb: "An ordinary, unhurried exchange in Japanese.",
        text: "ねえ、土曜日は本当に来られる？よかったら八時ごろ迎えに行くよ。チケットを\
               金曜までに買わないといけないから、明日までに連絡してね。正直、すごく楽しみに\
               してるんだ。",
    },
    Script {
        id: "zh-hans-everyday",
        title: "Everyday conversation",
        language: "zh-Hans",
        blurb: "An ordinary, unhurried exchange in Mandarin Chinese.",
        text: "你周六真的会来吗？要是方便的话，我八点左右过去接你，我们一起走。明天记得跟我说\
               一声，因为票得在星期五之前买好。说实话，我挺期待的，我们真的太久没见了。",
    },
];

/// The script with this id, if the catalog still has one.
///
/// Fallible on purpose: an id can outlive the entry it named — it travels
/// through a message, and a future version may drop a script — and a picker
/// that quietly falls back is better than one that panics.
#[must_use]
pub fn find(id: &str) -> Option<&'static Script> {
    SCRIPTS.iter().find(|s| s.id == id)
}

/// The base language subtag of a BCP-47 tag, lowercased: `pt-BR` → `pt`.
///
/// Matching is on the base alone. A Brazilian Portuguese script is a far better
/// reference for a `pt-PT` speaker than an English one, and the alternative —
/// exact-tag matching — would offer nothing at all to anyone whose model is
/// pinned to a region this catalog does not name.
fn base_language(tag: &str) -> Option<String> {
    if tag.eq_ignore_ascii_case("auto") {
        // A control, not a language: it says the daemon will decide per
        // utterance, which tells us nothing about what the user speaks.
        return None;
    }
    let base = tag.split('-').next().unwrap_or(tag).trim();
    (!base.is_empty()).then(|| base.to_lowercase())
}

/// Whether `script` is written in the language `tag` names.
///
/// A tag that names no language — `auto`, or an empty setting — counts as a
/// match: it says the daemon decides per utterance, which is no evidence that
/// the script is in the wrong language, and a warning drawn from it would fire
/// on every user who never set a Primary Language.
#[must_use]
pub fn same_language(script: &Script, tag: &str) -> bool {
    let Some(wanted) = base_language(tag) else {
        return true;
    };
    base_language(script.language).as_deref() == Some(wanted.as_str())
}

/// Every script, with the ones matching `preferred` first.
///
/// Ordered, never filtered. A user whose model speaks Japanese should be
/// offered the Japanese script first, but a user recording a voice for a
/// language this catalog has no script for must still see the rest rather than
/// an empty picker.
#[must_use]
pub fn offered(preferred: Option<&str>) -> Vec<&'static Script> {
    // A real base subtag or nothing: `auto` and an unset language name no
    // language to prefer, and must leave the catalog in its own order rather
    // than in one they did not choose.
    let base = preferred.and_then(base_language);
    let matches = |script: &Script| base.is_some() && base == base_language(script.language);
    let mut ordered: Vec<&'static Script> = SCRIPTS.iter().filter(|s| matches(s)).collect();
    ordered.extend(SCRIPTS.iter().filter(|s| !matches(s)));
    ordered
}

/// The script to offer when the user has not picked one.
///
/// The first one in the preferred language that the loaded model can hear all
/// of — a shorter script that arrives whole beats a richer one the model only
/// receives the first two thirds of. Falls back to the head of the list when
/// the budget is too small for anything, since an over-long script with a
/// warning under it still beats an empty picker.
#[must_use]
pub fn default_for(preferred: Option<&str>, budget: Option<f32>) -> Option<&'static Script> {
    let offered = offered(preferred);
    offered
        .iter()
        .copied()
        .find(|script| script.fits(budget))
        .or_else(|| offered.first().copied())
}

#[cfg(test)]
mod tests {
    use super::{SCRIPTS, Script, base_language, default_for, find, offered, same_language};

    /// The research this catalog is built on puts the useful range of a
    /// reference clip at roughly ten to thirty seconds: below it the model has
    /// too little to condition on, above it the gain has plateaued and the
    /// longer read is just a worse experience. A script that drifts out of the
    /// window as it is edited should fail here, not in someone's recording.
    #[test]
    fn every_script_reads_in_ten_to_thirty_seconds() {
        for script in SCRIPTS {
            let seconds = script.estimated_seconds();
            assert!(
                (10.0..=30.0).contains(&seconds),
                "{} reads in {seconds:.0} s, outside the 10-30 s window",
                script.id
            );
        }
    }

    #[test]
    fn ids_are_unique_and_texts_are_clean() {
        let mut ids: Vec<&str> = SCRIPTS.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate script id");

        for Script {
            id,
            title,
            language,
            blurb,
            text,
        } in SCRIPTS
        {
            assert!(!title.is_empty(), "{id} has no title");
            assert!(!blurb.is_empty(), "{id} has no blurb");
            assert!(!language.is_empty(), "{id} has no language");
            // A stray edge space would be read back as part of the transcript.
            assert_eq!(text.trim(), *text, "{id} has untrimmed text");
            // Line continuations in the source must not survive as newlines:
            // the transcript is a single utterance.
            assert!(!text.contains('\n'), "{id} spans lines");
        }
    }

    #[test]
    fn the_preferred_language_leads_the_list() {
        let spanish = offered(Some("es-MX"));
        assert_eq!(spanish[0].language, "es", "a regional tag matches its base");
        assert_eq!(spanish.len(), SCRIPTS.len(), "nothing is filtered away");

        let chinese = offered(Some("zh-Hans"));
        assert_eq!(chinese[0].language, "zh-Hans");

        // Portuguese is catalogued as pt-BR; a European tag still reaches it.
        assert_eq!(offered(Some("pt-PT"))[0].language, "pt-BR");
    }

    #[test]
    fn an_unknown_or_absent_language_falls_back_to_english() {
        for tag in [None, Some("auto"), Some("sw"), Some("")] {
            let first = default_for(tag, None).expect("the catalog is never empty");
            assert_eq!(first.language, "en", "for {tag:?}");
        }
    }

    /// Every language's default has to fit the tightest budget the shipped
    /// models declare — qwen3-tts takes 15 seconds — or the suggestion the page
    /// makes is one the model cannot hear the end of.
    #[test]
    fn every_language_has_a_default_that_fits_fifteen_seconds() {
        for tag in ["en", "es", "fr", "de", "it", "pt-BR", "hi", "ja", "zh-Hans"] {
            let script = default_for(Some(tag), Some(15.0)).expect("the catalog is never empty");
            assert_eq!(
                base_language(script.language),
                base_language(tag),
                "{tag} fell back to another language"
            );
            assert!(
                script.fits(Some(15.0)),
                "{}'s default reads in {:.0} s",
                tag,
                script.estimated_seconds()
            );
        }
    }

    /// A budget nothing fits still offers something, rather than a picker with
    /// no script in it.
    #[test]
    fn an_impossible_budget_still_suggests_a_script() {
        let script = default_for(Some("en"), Some(2.0)).expect("the catalog is never empty");
        assert!(!script.fits(Some(2.0)));
    }

    #[test]
    fn a_language_that_names_nothing_never_reads_as_a_mismatch() {
        let english = find("en-everyday").expect("catalogued above");
        assert!(same_language(english, "en-GB"), "a region is still English");
        assert!(!same_language(english, "ja"));
        // Nothing to disagree with: these say the language is undecided.
        assert!(same_language(english, "auto"));
        assert!(same_language(english, ""));
    }

    #[test]
    fn a_chosen_script_is_found_by_id_and_a_dropped_one_is_not() {
        assert_eq!(find("en-everyday"), Some(&SCRIPTS[0]));
        assert!(find("en-retired-in-some-future-version").is_none());
    }

    /// Japanese and Chinese carry no spaces, so a word count would time the
    /// whole passage as one word. The estimate has to see the characters.
    #[test]
    fn spaceless_scripts_are_timed_by_character() {
        let japanese = find("ja-everyday").expect("catalogued above");
        assert!(
            japanese.estimated_seconds() > 5.0,
            "counted as words, not characters"
        );
    }
}
