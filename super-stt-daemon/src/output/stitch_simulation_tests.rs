// SPDX-License-Identifier: GPL-3.0-only

//! Stitching under simulated sliding windows.
//!
//! The unit tests pin individual seams. This runs whole takes: a generated
//! passage is laid on a timeline, cut into the overlapping windows the preview
//! loop would read, roughed up the way a small model roughs up a window's
//! edges, and stitched. What comes out is scored against what went in, word
//! by word: words lost and words repeated. The bounds are regression guards;
//! run with `--nocapture` to read the rates when tuning the matcher.

use super::{merge_window_preview, normalize_text};

/// Characters of speech per second, for laying text on the timeline.
const CHARS_PER_SECOND: f64 = 15.0;
/// The preview loop's window sizing: the gap since the last pass plus the
/// shared overlap, clamped.
const OVERLAP_SECS: f64 = 3.0;
const MIN_WINDOW_SECS: f64 = 5.0;
const MAX_WINDOW_SECS: f64 = 15.0;

/// How often a rough model swaps a word inside a window for another one.
/// Whisper tiny does this constantly at a window's edges.
const ROUGH_REWORD_CHANCE: f64 = 0.25;
/// How often a word cut by the window's edge is dropped rather than kept as
/// a fragment.
const DROP_CUT_WORD_CHANCE: f64 = 0.7;

/// A small deterministic generator so every run sees the same takes.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n as u64).expect("fits")
    }

    fn chance(&mut self, p: f64) -> bool {
        // reason: a test PRNG; the truncation is the point.
        #[allow(clippy::cast_precision_loss)]
        let unit = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
        unit < p
    }

    fn between(&mut self, lo: f64, hi: f64) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let unit = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
        lo + (hi - lo) * unit
    }
}

/// English words by how often they are spoken, from the OpenSubtitles corpus
/// (see `tests/fixtures/words/SOURCE.md`). Sampling by count keeps the short
/// function words as common as they are in speech, and a false seam is made
/// of exactly those lining up by coincidence.
const WORD_LIST: &str = include_str!("../../tests/fixtures/words/en_10k.txt");

/// The words and, for sampling, the running total of their counts.
struct Vocabulary {
    words: Vec<&'static str>,
    cumulative: Vec<u64>,
}

impl Vocabulary {
    fn load() -> Self {
        let mut words = Vec::new();
        let mut cumulative = Vec::new();
        let mut total = 0u64;
        for line in WORD_LIST.lines() {
            let mut fields = line.split_whitespace();
            let (Some(word), Some(count)) = (fields.next(), fields.next()) else {
                continue;
            };
            // Contraction pieces ("'s", "'t") are tokens, not words.
            if !word.chars().all(|c| c.is_ascii_alphabetic()) {
                continue;
            }
            let count: u64 = count.parse().expect("a count per line");
            total += count;
            words.push(word);
            cumulative.push(total);
        }
        assert!(words.len() > 1000, "the word list loaded");
        Self { words, cumulative }
    }

    /// A word, as likely as it is in speech.
    fn pick(&self, rng: &mut Rng) -> &'static str {
        let total = *self.cumulative.last().expect("non-empty");
        let target = rng.next() % total;
        let i = self.cumulative.partition_point(|&c| c <= target);
        self.words[i.min(self.words.len() - 1)]
    }
}

/// A passage of `words` words in sentences of six to fourteen, lowercase,
/// each sentence closed with a period.
fn passage(rng: &mut Rng, vocabulary: &Vocabulary, words: usize) -> String {
    let mut out = String::new();
    let mut in_sentence = 0;
    let mut sentence_len = 6 + rng.below(9);
    for i in 0..words {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(vocabulary.pick(rng));
        in_sentence += 1;
        if in_sentence == sentence_len && i + 1 < words {
            out.push('.');
            in_sentence = 0;
            sentence_len = 6 + rng.below(9);
        }
    }
    out.push('.');
    out
}

/// The text a model would return for the audio between chars `from` and
/// `to` of the passage: cut words at the edges dropped or kept as fragments,
/// a word swapped now and then, the first word capitalized, sometimes a
/// closing period.
fn window(
    rng: &mut Rng,
    vocabulary: &Vocabulary,
    chars: &[char],
    from: usize,
    to: usize,
    reword: f64,
) -> String {
    let slice: String = chars[from..to].iter().collect();
    let cut_at_start = from > 0 && chars[from - 1] != ' ' && chars[from] != ' ';
    let cut_at_end = to < chars.len() && chars[to] != ' ' && chars[to - 1] != ' ';
    let mut words: Vec<String> = slice
        .split_whitespace()
        .map(|w| w.trim_matches('.').to_string())
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        return String::new();
    }
    if cut_at_start && rng.chance(DROP_CUT_WORD_CHANCE) {
        words.remove(0);
    }
    if cut_at_end && !words.is_empty() && rng.chance(DROP_CUT_WORD_CHANCE) {
        words.pop();
    }
    if words.is_empty() {
        return String::new();
    }
    if rng.chance(reword) {
        let i = rng.below(words.len());
        words[i] = vocabulary.pick(rng).to_string();
    }
    let mut text = words.join(" ");
    if rng.chance(0.5) {
        text.push('.');
    }
    // The model capitalizes a window's first word; sometimes it does not.
    if rng.chance(0.8) {
        let mut cs = text.chars();
        text = cs
            .next()
            .map(|c| c.to_ascii_uppercase().to_string() + cs.as_str())
            .unwrap_or_default();
    }
    text
}

/// One take: the passage read through windows at roughly `interval` seconds
/// apart. Returns the words that were spoken up to the last window's end and
/// the words the stitched transcript ended up with.
fn simulate(
    seed: u64,
    vocabulary: &Vocabulary,
    interval_secs: f64,
    words: usize,
    reword: f64,
) -> (Vec<String>, Vec<String>) {
    let mut rng = Rng(seed);
    let text = passage(&mut rng, vocabulary, words);
    let chars: Vec<char> = text.chars().collect();
    // reason: a test timeline; the count fits comfortably.
    #[allow(clippy::cast_precision_loss)]
    let total_secs = chars.len() as f64 / CHARS_PER_SECOND;

    let mut session = String::new();
    let mut prev = String::new();
    let mut last_pass = 0.0_f64;
    let mut now = interval_secs;
    let mut spoken_to = 0;
    while now < total_secs {
        let gap = now - last_pass;
        let window_secs = (gap + OVERLAP_SECS).clamp(MIN_WINDOW_SECS, MAX_WINDOW_SECS);
        // reason: timeline maths on small positive numbers.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let to = ((now * CHARS_PER_SECOND) as usize).min(chars.len());
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let from = ((now - window_secs).max(0.0) * CHARS_PER_SECOND) as usize;
        spoken_to = to;

        let normalized = normalize_text(&window(&mut rng, vocabulary, &chars, from, to, reword));
        if !normalized.is_empty() && normalized != prev {
            session = merge_window_preview(&session, &normalized);
            prev = normalized;
        }

        last_pass = now;
        now += interval_secs * rng.between(0.8, 1.6);
    }

    let spoken: String = chars[..spoken_to].iter().collect();
    (words_of(&spoken), words_of(&session))
}

fn words_of(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// Length of the longest common subsequence of two word lists.
fn common_words(a: &[String], b: &[String]) -> usize {
    let mut prev = vec![0usize; b.len() + 1];
    let mut cur = vec![0usize; b.len() + 1];
    for x in a {
        for (j, y) in b.iter().enumerate() {
            cur[j + 1] = if x == y {
                prev[j] + 1
            } else {
                cur[j].max(prev[j + 1])
            };
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

struct Score {
    spoken: usize,
    lost: usize,
    repeated: usize,
}

fn score(seeds: std::ops::Range<u64>, interval_secs: f64, reword: f64) -> Score {
    let vocabulary = Vocabulary::load();
    let mut total = Score {
        spoken: 0,
        lost: 0,
        repeated: 0,
    };
    for seed in seeds {
        let words = 60 + usize::try_from(seed % 60).expect("small");
        let (spoken, stitched) = simulate(seed, &vocabulary, interval_secs, words, reword);
        let common = common_words(&spoken, &stitched);
        total.spoken += spoken.len();
        total.lost += spoken.len() - common;
        total.repeated += stitched.len() - common;
    }
    total
}

// reason: rates for a human to read; precision is irrelevant.
#[allow(clippy::cast_precision_loss)]
fn rate(part: usize, whole: usize) -> f64 {
    part as f64 / whole.max(1) as f64
}

fn report(label: &str, s: &Score) {
    eprintln!(
        "{label}: {} spoken, lost {:.1}%, repeated {:.1}%",
        s.spoken,
        100.0 * rate(s.lost, s.spoken),
        100.0 * rate(s.repeated, s.spoken)
    );
}

/// A model that never rewords leaves exact seams: the transcript is the
/// passage, give or take a word cut at the edge of the last window.
#[test]
fn an_exact_model_reproduces_the_passage() {
    let s = score(1..41, 2.0, 0.0);
    report("exact model", &s);
    assert!(
        rate(s.lost, s.spoken) < 0.01,
        "lost {} of {}",
        s.lost,
        s.spoken
    );
    assert!(
        rate(s.repeated, s.spoken) < 0.01,
        "repeated {} of {}",
        s.repeated,
        s.spoken
    );
}

/// A rough, fast model reads a window every second, so consecutive windows
/// share almost everything; a swapped word inside the shared stretch is
/// outvoted by the transcript's own reading. With the seam anchors at 24
/// characters this repeated a quarter of every take.
#[test]
fn a_rough_fast_model_loses_little_and_repeats_little() {
    let s = score(1..41, 1.0, ROUGH_REWORD_CHANCE);
    report("rough fast model", &s);
    // A swapped word in a window's new tail is a legitimate part of the
    // transcript yet counts as lost here, so a few percent is the floor.
    assert!(
        rate(s.lost, s.spoken) < 0.06,
        "lost {} of {}",
        s.lost,
        s.spoken
    );
    assert!(
        rate(s.repeated, s.spoken) < 0.06,
        "repeated {} of {}",
        s.repeated,
        s.spoken
    );
}

/// A rough, slow model reads a window every five seconds, so each window is
/// mostly new text and the three-second overlap is all there is to stitch on.
#[test]
fn a_rough_slow_model_loses_little_and_repeats_little() {
    let s = score(1..41, 5.0, ROUGH_REWORD_CHANCE);
    report("rough slow model", &s);
    assert!(
        rate(s.lost, s.spoken) < 0.06,
        "lost {} of {}",
        s.lost,
        s.spoken
    );
    assert!(
        rate(s.repeated, s.spoken) < 0.06,
        "repeated {} of {}",
        s.repeated,
        s.spoken
    );
}
