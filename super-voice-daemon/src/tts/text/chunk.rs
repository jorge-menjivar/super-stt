// SPDX-License-Identifier: GPL-3.0-only
//! Splitting normalized text into synthesis chunks.
//!
//! # Why chunk at all
//!
//! Two reasons, and they pull in opposite directions.
//!
//! A model may declare `max_input_chars`, and text over it is simply refused —
//! most local models have a hard input length. That is a correctness bound.
//!
//! The other reason is latency: nothing is heard until the first chunk comes
//! back, so a short first chunk gets speech started while the rest is still
//! synthesizing. But every extra split costs a round trip and resets the
//! model's prosodic context, which is audible as a flatter reading. So the
//! policy here is deliberately asymmetric — one small chunk to kill dead air,
//! then as few splits as the model's limit allows.
//!
//! # Sentence boundaries
//!
//! Splitting mid-sentence sounds wrong in a way splitting between sentences
//! does not, so the chunker only ever cuts at a boundary — except when a single
//! "sentence" exceeds the model's limit, where it falls back to a word break
//! rather than refusing to speak.
//!
//! Finding those boundaries is most of the work, and getting it wrong is
//! audible: "Dr. Smith arrived" split after `Dr.` produces two utterances with
//! a pause and a dropped intonation. See `boundary_end`.
//!
//! # Line breaks
//!
//! The [normalizer](super::normalize) keeps a line break that ends a line — a
//! heading, a list item, a paragraph — as a `\n`, and here it is a boundary
//! of a stronger kind than a period. A period is only a *candidate* split: the
//! chunker packs sentences together until the target is outgrown, because the
//! model reads the pause in the punctuation itself. A line break carries no
//! pause the model can read — a backend never sees one — so the text either
//! side of it is cut apart as soon as the break is known, whatever the
//! target. A list of five items is five short utterances, which is what it
//! looks like.
//!
//! # Streaming and one-shot must agree
//!
//! The daemon's one-shot path is the streaming one with the whole string
//! pushed at once, so the same text must chunk the same way however it is cut
//! into deltas — otherwise `POST /speak` and `GET /speak/stream` read the same
//! sentence with different phrasing, and which one you get depends on the
//! producer's token sizes.
//!
//! That does not come for free. A terminator is judged by what *follows* it,
//! so a delta ending just after one leaves the question open, and answering it
//! early — in either direction — breaks the invariant. Hence the chunker's
//! boundary verdict has three states rather than two: "not a boundary" means
//! look further back, "cannot tell yet" means wait for the next delta.

/// Sentence-ending punctuation.
const TERMINATORS: [char; 6] = ['.', '!', '?', '。', '！', '？'];

/// Words that end in `.` without ending a sentence.
///
/// Lower-cased for comparison. This list is short on purpose: every entry is a
/// judgement that the word is more often an abbreviation than a sentence end,
/// and a wrong entry silently merges two sentences, which is a milder failure
/// than splitting one.
const ABBREVIATIONS: &[&str] = &[
    "mr", "mrs", "ms", "dr", "prof", "sr", "jr", "st", "mt", "rev", "hon", "vs", "etc", "eg", "ie",
    "al", "fig", "no", "vol", "pp", "approx", "inc", "ltd", "co", "corp", "dept", "est", "min",
    "max", "cf", "ca", "circa", "jan", "feb", "mar", "apr", "jun", "jul", "aug", "sep", "sept",
    "oct", "nov", "dec", "mon", "tue", "wed", "thu", "fri", "sat", "sun",
];

/// How chunking behaves for one model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkPolicy {
    /// Hard limit from the model's manifest. `None` means unbounded, and the
    /// chunker then emits one chunk per utterance apart from the opening one.
    pub max_chars: Option<usize>,
    /// Target size for the first chunk of an utterance. Kept small so speech
    /// starts quickly; `0` disables the short opener.
    pub first_chunk_chars: usize,
}

impl Default for ChunkPolicy {
    fn default() -> Self {
        Self {
            max_chars: None,
            // Roughly one short sentence — enough to carry a clause, small
            // enough that the first synthesis returns fast.
            first_chunk_chars: 120,
        }
    }
}

impl ChunkPolicy {
    /// A policy for a model declaring `max_input_chars`.
    #[must_use]
    pub fn for_model(max_chars: Option<u32>) -> Self {
        Self {
            max_chars: max_chars.map(|c| c as usize),
            ..Self::default()
        }
    }

    /// The size this chunk is aiming for, given how many came before.
    fn target(self, emitted: usize) -> usize {
        let cap = self.max_chars.unwrap_or(usize::MAX);
        if emitted == 0 && self.first_chunk_chars > 0 {
            self.first_chunk_chars.min(cap)
        } else {
            cap
        }
    }
}

/// Streaming sentence chunker.
#[derive(Debug)]
pub struct Chunker {
    policy: ChunkPolicy,
    buf: String,
    emitted: usize,
}

impl Chunker {
    /// A chunker following `policy`.
    #[must_use]
    pub fn new(policy: ChunkPolicy) -> Self {
        Self {
            policy,
            buf: String::new(),
            emitted: 0,
        }
    }

    /// Feed normalized text; returns every chunk that is now complete.
    pub fn push(&mut self, text: &str) -> Vec<String> {
        self.buf.push_str(text);
        self.drain(false)
    }

    /// Flush whatever remains as a final chunk.
    pub fn finish(&mut self) -> Vec<String> {
        let mut out = self.drain(true);
        let rest = self.buf.trim().to_string();
        self.buf.clear();
        self.emitted = 0;
        if !rest.is_empty() {
            out.push(rest);
        }
        out
    }

    /// Pull every chunk the buffer already holds.
    ///
    /// `final_pass` means no more input is coming, so a candidate boundary
    /// that is only waiting on the text after it is settled here rather than
    /// held — the same split [`Normalizer`](super::normalize::Normalizer)
    /// makes for the same reason.
    fn drain(&mut self, final_pass: bool) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(chunk) = self.take_ready(final_pass) {
            out.push(chunk);
        }
        out
    }

    /// Pull one chunk if the buffer holds a complete one.
    fn take_ready(&mut self, final_pass: bool) -> Option<String> {
        // Leading whitespace is never part of a chunk; dropping it here keeps
        // a line break that arrives at the head of the buffer from being cut
        // off as an empty chunk.
        if self.buf.starts_with(char::is_whitespace) {
            self.buf = self.buf.trim_start().to_string();
        }
        let target = self.policy.target(self.emitted);
        let len = self.buf.chars().count();

        // A line break ends the chunk where it stands, target or no target —
        // see the module doc. A line longer than the model's limit is cut at
        // the limit first, below, and reaches this on a later pass.
        if let Some(pos) = self.buf.chars().position(|c| c == '\n')
            && self.policy.max_chars.is_none_or(|max| pos <= max)
        {
            return self.split_at(pos + 1);
        }

        // Nothing is emitted until the buffer has actually outgrown its
        // target. Splitting at every boundary the moment one appears would
        // turn "Hello there. How are you?" into two requests for no reason —
        // and each extra split costs a round trip and resets the model's
        // prosodic context. It is also half of what makes streaming and
        // one-shot agree: both *fire* on the same content-based condition
        // rather than on when the text happened to arrive. The other half is
        // the `NeedMore` arm below, which keeps them picking the same split.
        if len <= target {
            return None;
        }

        // Prefer the last sentence boundary at or before the target: it packs
        // the chunk as full as the policy allows without overshooting.
        let split = match last_boundary_within(&self.buf, target, final_pass) {
            Boundary::At(split) => split,
            // A terminator sits in the window, but what would settle it — the
            // space and the capital that tell a sentence end from `etc.` — has
            // not arrived. Settling for an earlier boundary is what used to
            // make this path disagree with a one-shot caller: that caller has
            // the whole string, judges this same candidate, and packs a fuller
            // chunk, so identical text split into different deltas produced
            // different chunks. Waiting costs one delta. `final_pass` is
            // already folded into the verdict, so this cannot hold at `finish`.
            Boundary::NeedMore => return None,
            // No boundary fits. Only force a split when the text has actually
            // outgrown the hard limit — otherwise wait for more input rather
            // than cutting mid-sentence.
            Boundary::No => match self.policy.max_chars {
                Some(max) if len > max => word_break_before(&self.buf, max),
                _ => return None,
            },
        };

        self.split_at(split)
    }

    /// Take the first `split` characters of the buffer as a chunk.
    fn split_at(&mut self, split: usize) -> Option<String> {
        let head: String = self.buf.chars().take(split).collect();
        let rest: String = self.buf.chars().skip(split).collect();
        let chunk = head.trim().to_string();
        self.buf = rest.trim_start().to_string();
        if chunk.is_empty() {
            return None;
        }
        self.emitted += 1;
        Some(chunk)
    }
}

/// What can be concluded about a sentence boundary — about one candidate
/// terminator in [`boundary_end`], and about a whole window in
/// [`last_boundary_within`].
///
/// [`NeedMore`](Boundary::NeedMore) is the variant that earns the enum. A
/// terminator is judged by what follows it, so at the end of the buffer there
/// is nothing to judge with, and "not a boundary" and "cannot tell yet" are
/// different answers: the first means look further back, the second means wait.
/// Collapsing them into `None` made the chunker fall back to an earlier
/// boundary — or, past a model's limit, to a mid-sentence word break — for text
/// a one-shot caller split correctly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Boundary {
    /// A sentence ends here; the split lands at this character index.
    At(usize),
    /// Not a sentence end: a decimal point, an abbreviation, `example.com`.
    No,
    /// Undecidable from the text so far. Only reachable mid-stream — a final
    /// pass has, by definition, all the text there will ever be.
    NeedMore,
}

/// The last sentence boundary at or before `limit`.
///
/// `final_pass` means no more text is coming, which settles every candidate:
/// one that merely lacks following text does not end a sentence *here*, and
/// [`Chunker::finish`] flushes the remainder wholesale instead.
fn last_boundary_within(s: &str, limit: usize, final_pass: bool) -> Boundary {
    let chars: Vec<char> = s.chars().collect();
    let end = limit.min(chars.len());
    // Scanned in reverse so the first decisive answer is also the latest
    // boundary, which packs the chunk as full as the policy allows. The split
    // lands past any closing quote, not straight after the terminator —
    // otherwise `He said "go now."` is cut before its own closing quote, which
    // then opens the next chunk.
    for i in (0..end).rev() {
        match boundary_end(&chars, i) {
            Boundary::At(split) => return Boundary::At(split),
            // Undecidable, and this candidate sits later than anything still
            // to be scanned — so whatever it turns out to be beats an earlier
            // boundary. Wait instead of settling.
            Boundary::NeedMore if !final_pass => return Boundary::NeedMore,
            Boundary::NeedMore | Boundary::No => {}
        }
    }
    Boundary::No
}

/// Whether `chars[i]` ends a sentence.
///
/// The cases that matter, each of which is audible when it goes wrong:
///
/// - **Decimals and versions.** The `.` in `3.14` or `v1.2` has digits on both
///   sides. Splitting there says "three point" and then "one four".
/// - **Abbreviations.** `Dr.`, `etc.`, `Fig.` — see [`ABBREVIATIONS`].
/// - **Initials.** A single capital before the `.`, as in `J. R. R. Tolkien`.
/// - **Ellipses.** `...` is a pause inside a sentence far more often than an
///   end, so only the last dot of a run is even considered.
/// - **What follows.** A real boundary is followed by whitespace and then
///   something that can start a sentence. `example.com` fails this because `c`
///   is neither.
fn boundary_end(chars: &[char], i: usize) -> Boundary {
    let c = chars[i];
    // A line break needs nothing after it to be judged by: the normalizer
    // already did that, and only kept the ones that end a line.
    if c == '\n' {
        return Boundary::At(i + 1);
    }
    if !TERMINATORS.contains(&c) {
        return Boundary::No;
    }

    // Trailing quotes and brackets belong to the sentence that is ending, so
    // step over them *before* judging what follows — and the split lands past
    // them. Checking `chars[i + 1]` directly would see the closing quote in
    // `"go now." Then` and conclude the dot was mid-token, like `example.com`.
    let mut j = i + 1;
    while chars
        .get(j)
        .is_some_and(|c| matches!(c, '"' | '\'' | '”' | '’' | ')' | ']'))
    {
        j += 1;
    }

    // CJK terminators are unambiguous: never decimal points, never in
    // abbreviations, never in domain names.
    if matches!(c, '。' | '！' | '？') {
        return Boundary::At(j);
    }

    if c == '.' {
        let prev = i.checked_sub(1).map(|p| chars[p]);
        let after = chars.get(j).copied();

        // A decimal point, or a dot inside a version number.
        if prev.is_some_and(|p| p.is_ascii_digit()) && after.is_some_and(|n| n.is_ascii_digit()) {
            return Boundary::No;
        }
        // Mid-ellipsis: only the final dot of a run is even a candidate.
        if after == Some('.') {
            return Boundary::No;
        }
        // A dot glued to what follows: `example.com`, `a.b`.
        if after.is_some_and(|n| !n.is_whitespace()) {
            return Boundary::No;
        }
        // A single capital before the dot is an initial: `J. R. R. Tolkien`.
        let word = word_before(chars, i);
        if word.chars().count() == 1 && word.chars().next().is_some_and(char::is_uppercase) {
            return Boundary::No;
        }
        if ABBREVIATIONS.contains(&word.to_lowercase().as_str()) {
            return Boundary::No;
        }
    }

    match chars.get(j) {
        // The buffer ends here, so there is nothing to judge the terminator
        // by. Whether it ends a sentence depends on text that has not arrived;
        // `finish` resolves it by flushing the remainder wholesale.
        None => Boundary::NeedMore,
        Some(n) if !n.is_whitespace() => Boundary::No,
        // What follows has to look like the start of a sentence. This is the
        // general form of the abbreviation list: no enumeration can cover
        // `e.g.`, `p.m.`, or a name nobody thought of, but a lowercase word
        // after a period almost never starts one. It also settles the ellipsis
        // case, where "Well... it depends" continues rather than restarts.
        _ => match starts_a_sentence(chars, j) {
            Some(true) => Boundary::At(j),
            Some(false) => Boundary::No,
            // Trailing whitespace and nothing else yet: the same as running
            // out at `j`, one delta later.
            None => Boundary::NeedMore,
        },
    }
}

/// Whether the first non-space character at or after `j` could open a sentence.
///
/// Uppercase, a digit, an opening quote or bracket — or any character from a
/// script without letter case, where the test does not apply and a terminator
/// is already unambiguous.
///
/// `None` when there is no such character *yet*: everything from `j` on is
/// whitespace, so the question cannot be answered until more text arrives.
/// Answering `false` there is what cut `…Six. Seven. | Eight…` one sentence
/// short whenever a delta happened to end on that space.
fn starts_a_sentence(chars: &[char], j: usize) -> Option<bool> {
    let &c = chars[j..].iter().find(|c| !c.is_whitespace())?;
    if c.is_lowercase() {
        return Some(false);
    }
    Some(
        c.is_uppercase()
            || c.is_ascii_digit()
            || matches!(c, '"' | '\'' | '“' | '‘' | '(' | '[')
            // Caseless scripts (CJK, Arabic, Hebrew, …): `is_lowercase` and
            // `is_uppercase` are both false, and treating that as "not a
            // sentence" would merge every sentence in those languages into
            // one chunk.
            || !c.is_alphabetic()
            || (!c.is_lowercase() && !c.is_uppercase()),
    )
}

/// The word immediately before `chars[i]`, without its punctuation.
fn word_before(chars: &[char], i: usize) -> String {
    let mut start = i;
    while start > 0 {
        let c = chars[start - 1];
        if c.is_alphanumeric() {
            start -= 1;
        } else {
            break;
        }
    }
    chars[start..i].iter().collect()
}

/// Character index of a word break at or before `limit`, for text with no
/// usable sentence boundary. Falls back to a hard cut when a single word is
/// longer than the limit, since refusing to speak is worse.
fn word_break_before(s: &str, limit: usize) -> usize {
    let chars: Vec<char> = s.chars().collect();
    let end = limit.min(chars.len());
    (0..end)
        .rev()
        .find(|&i| chars[i].is_whitespace())
        .map_or(end, |i| i + 1)
}

#[cfg(test)]
mod tests;
