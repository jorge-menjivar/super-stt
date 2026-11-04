// SPDX-License-Identifier: GPL-3.0-only
//! Turning screen and LLM text into something worth speaking.
//!
//! # What this does and does not do
//!
//! It strips **markup that would otherwise be read out literally** — asterisks,
//! backticks, link syntax, heading hashes, code fences — and collapses
//! whitespace, keeping one thing the whitespace said: a line break that ends a
//! line by the look of the page (see [Line breaks](#line-breaks)). It
//! deliberately does **not** convert numbers, dates, currency,
//! or units into words. Every TTS model in scope already reads `1,234.56` and
//! `2026-08-29` correctly, and number-to-words is language-specific enough that
//! a wrong implementation is worse than none. What matters here is not
//! *breaking* those spellings — which is why the [chunker](super::chunk) knows
//! that the `.` in `3.14` is not a sentence boundary.
//!
//! # Why it streams
//!
//! Text arrives from an LLM a few tokens at a time, and markup is ambiguous
//! until more of it shows up: a trailing `*` might open emphasis or start a
//! bullet, a trailing `` ` `` might open inline code or be the first character
//! of a fence delimiter. Deciding early and being wrong speaks a literal asterisk
//! — the exact failure the normalizer exists to prevent.
//!
//! So [`Normalizer::push`] returns only text whose meaning can no longer
//! change, and holds the ambiguous tail until the next delta resolves it or
//! [`Normalizer::finish`] declares there is no more. Callers that have the whole
//! string up front use [`normalize`], which is just push-then-finish.
//!
//! # Line breaks
//!
//! A heading, a list item, a paragraph: on the page each ends where its line
//! does, whether or not the author put a period there, and read aloud they end
//! there too. Collapsing every newline to a space handed the model `Summary
//! Everything passed Next steps` as one sentence, and it read it as one. So a
//! line break that ends something is kept as a single `\n`, which the
//! [chunker](super::chunk) treats as a boundary; a line break that merely
//! wraps a sentence — the other thing a newline means, in hard-wrapped prose —
//! collapses to a space as before.
//!
//! Telling the two apart is a judgement from what surrounds the break, and the
//! rule is kept small. The break is a real one when the page says so: a blank
//! line, a heading on either side, a list item, a quote or a fence on the next
//! line. Otherwise it is one when the next line starts the way a sentence does —
//! anything but a lowercase letter, after any opening markup or quote — unless
//! the line before it ends in a comma or semicolon, which no sentence does. A
//! wrapped line that happens to begin with a name is the case this gets wrong,
//! and it is accepted: text from an LLM, which is what streams through here, is
//! never hard-wrapped, and a screen selection that is loses one pause where a
//! run-on list lost every one.

/// Spoken in place of a fenced code block.
///
/// Reading code aloud character by character is useless, and dropping it
/// silently loses the fact that something was there. Naming it is the honest
/// middle.
const CODE_BLOCK_NOTICE: &str = "code block";

/// Longest tail held while waiting for markup to resolve.
///
/// Without a cap, a single unterminated backtick would swallow the rest of the
/// utterance: the normalizer would keep holding, waiting for a close that never
/// comes, and nothing would ever be spoken.
const MAX_HELD: usize = 4096;

/// Streaming markup stripper.
///
/// Feed deltas with [`push`](Self::push); call [`finish`](Self::finish) when the
/// text ends.
#[derive(Debug, Default)]
pub struct Normalizer {
    /// Text received but not yet resolvable.
    held: String,
    /// Inside a fenced block: everything is dropped until the fence closes.
    in_fence: bool,
    /// Whitespace collapsed but not yet written, so runs collapse across delta
    /// boundaries as well as within one: a space, or a line break that ends a
    /// line (see [Line breaks](self#line-breaks)).
    pending: Pending,
    /// Whether anything has been emitted yet (suppresses a leading space).
    emitted: bool,
    /// Whether the line being emitted opened with heading marks, which makes
    /// the break that ends it a real one whatever follows.
    line_is_heading: bool,
    /// The last character emitted, for the break rule: a line ending in a
    /// comma or semicolon continues on the next.
    last_char: Option<char>,
}

/// Collapsed whitespace waiting for the text that follows it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Pending {
    #[default]
    None,
    /// Written as one space.
    Space,
    /// Written as one `\n`: the line before it ended.
    Break,
}

impl Normalizer {
    /// A normalizer with nothing held.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a delta; returns text that is safe to speak.
    ///
    /// May return an empty string when everything so far is still ambiguous.
    pub fn push(&mut self, delta: &str) -> String {
        self.held.push_str(delta);
        self.drain(false)
    }

    /// Flush the tail: nothing more is coming, so ambiguity resolves to
    /// "literal text".
    pub fn finish(&mut self) -> String {
        let out = self.drain(true);
        self.in_fence = false;
        self.emitted = false;
        self.pending = Pending::None;
        self.line_is_heading = false;
        self.last_char = None;
        out
    }

    /// Consume as much of `held` as can be resolved.
    ///
    /// `final_pass` means no more input is coming, so a construct that is still
    /// open (an unclosed fence, a dangling `*`) is emitted as plain text rather
    /// than held forever.
    fn drain(&mut self, final_pass: bool) -> String {
        let mut out = String::new();
        // Over the cap the tail cannot be ambiguity any more — it is an
        // unterminated construct, and holding it forever would mean silence.
        let forced = final_pass || self.held.len() > MAX_HELD;

        loop {
            if self.in_fence {
                if let Some(end) = find_fence_close(&self.held) {
                    self.held.drain(..end);
                    self.in_fence = false;
                    self.push_word(&mut out, CODE_BLOCK_NOTICE);
                    continue;
                }
                if forced {
                    // An unclosed fence still had a code block in it.
                    self.held.clear();
                    self.in_fence = false;
                    self.push_word(&mut out, CODE_BLOCK_NOTICE);
                }
                break;
            }

            let Some(step) = self.next_step(forced) else {
                break;
            };
            match step {
                Step::Emit(text, consumed) => {
                    self.push_text(&mut out, &text);
                    self.held.drain(..consumed);
                }
                Step::Skip(consumed) => {
                    self.held.drain(..consumed);
                }
                Step::Space(consumed) => {
                    if self.pending != Pending::Break {
                        self.pending = Pending::Space;
                    }
                    self.held.drain(..consumed);
                }
                Step::Break(consumed) => {
                    self.pending = Pending::Break;
                    self.line_is_heading = false;
                    self.held.drain(..consumed);
                }
                Step::Heading(consumed) => {
                    self.line_is_heading = true;
                    self.held.drain(..consumed);
                }
                Step::OpenFence(consumed) => {
                    self.held.drain(..consumed);
                    self.in_fence = true;
                }
                Step::Hold => break,
            }
        }
        out
    }

    /// Decide what to do with the front of `held`.
    #[allow(clippy::too_many_lines)]
    fn next_step(&self, forced: bool) -> Option<Step> {
        let s: &str = &self.held;
        let mut chars = s.char_indices();
        let (_, first) = chars.next()?;
        let rest = &s[first.len_utf8()..];

        // Whitespace: collapse a run. Held unless something follows, so a
        // trailing space does not commit before the next delta arrives (which
        // may continue a word) — and so a line break is judged by the line
        // after it.
        if first.is_whitespace() {
            let end = s
                .char_indices()
                .find(|(_, c)| !c.is_whitespace())
                .map_or(s.len(), |(i, _)| i);
            if end == s.len() && !forced {
                return Some(Step::Hold);
            }
            let newlines = s[..end].chars().filter(|c| *c == '\n').count();
            if newlines == 0 {
                return Some(Step::Space(end));
            }
            return Some(match self.line_ends(newlines, &s[end..], forced) {
                Some(true) => Step::Break(end),
                Some(false) => Step::Space(end),
                None => Step::Hold,
            });
        }

        // Fences. ``` needs three backticks to be sure; fewer is inline code.
        if first == '`' {
            let ticks = s.chars().take_while(|c| *c == '`').count();
            if ticks >= 3 {
                // Skip the info string (`rust`, `python`) up to the newline.
                let after = s[ticks..].find('\n').map(|i| ticks + i + 1);
                return match after {
                    Some(end) => Some(Step::OpenFence(end)),
                    None if forced => Some(Step::OpenFence(s.len())),
                    None => Some(Step::Hold),
                };
            }
            if ticks < 3 && s.len() < 3 && !forced {
                // Could still become a fence once more arrives.
                return Some(Step::Hold);
            }
            // Inline code: drop the delimiter, speak the contents.
            return Some(Step::Skip(ticks));
        }

        // Emphasis and strikethrough delimiters carry no sound.
        if matches!(first, '*' | '_' | '~') {
            let run = s.chars().take_while(|c| *c == first).count();
            if run == s.len() && !forced {
                return Some(Step::Hold);
            }
            return Some(Step::Skip(run));
        }

        // Links: `[label](target)` speaks the label. The target is a URL the
        // listener cannot use, and reading it is noise.
        if first == '[' {
            return Some(match parse_link(s) {
                Some((label, consumed)) => Step::Emit(label, consumed),
                None if forced => Step::Skip(1),
                None => Step::Hold,
            });
        }

        // Structural markers only meaningful at the start of a line.
        if self.at_line_start() {
            if first == '#' {
                let hashes = s.chars().take_while(|c| *c == '#').count();
                if hashes < s.len() || forced {
                    return Some(Step::Heading(hashes));
                }
                return Some(Step::Hold);
            }
            if first == '>' {
                return Some(Step::Skip(1));
            }
            // `- item` / `* item` / `+ item`: a bullet, not emphasis. The
            // trailing space is what distinguishes them, so wait for it.
            if matches!(first, '-' | '+') {
                return Some(match rest.chars().next() {
                    Some(c) if c.is_whitespace() => Step::Skip(1),
                    Some(_) => Step::Emit(first.to_string(), first.len_utf8()),
                    None if forced => Step::Emit(first.to_string(), first.len_utf8()),
                    None => Step::Hold,
                });
            }
        }

        // A bare URL: say the host and drop the path, which is unspeakable.
        if s.starts_with("http://") || s.starts_with("https://") || s.starts_with("www.") {
            let end = s
                .char_indices()
                .find(|(_, c)| c.is_whitespace())
                .map_or(s.len(), |(i, _)| i);
            if end == s.len() && !forced {
                return Some(Step::Hold);
            }
            return Some(Step::Emit(spoken_url(&s[..end]), end));
        }

        // Symbols with no pronunciation. Kept narrow on purpose: stripping
        // anything "unusual" would mangle non-Latin scripts.
        if is_unspeakable(first) {
            return Some(Step::Skip(first.len_utf8()));
        }

        // Ordinary text up to the next character that might mean something.
        let end = s
            .char_indices()
            .skip(1)
            .find(|(_, c)| c.is_whitespace() || is_markup(*c))
            .map_or(s.len(), |(i, _)| i);
        if end == s.len() && !forced {
            // The word may continue into the next delta.
            return Some(Step::Hold);
        }
        Some(Step::Emit(s[..end].to_string(), end))
    }

    /// Whether the line break ending a run of `newlines` newlines ends the
    /// line, given `after`, the text that follows the run — see
    /// [Line breaks](self#line-breaks). `None` when `after` is too short to
    /// say and more may come.
    fn line_ends(&self, newlines: usize, after: &str, forced: bool) -> Option<bool> {
        // A blank line ends a paragraph; a heading ends with its line.
        if newlines >= 2 || self.line_is_heading {
            return Some(true);
        }
        let mut chars = after.chars();
        let first = chars.next()?;
        match first {
            // A heading or a quote starts the next line.
            '#' | '>' => Some(true),
            // A fence does; a shorter run of backticks is inline code, judged
            // like any other start of a line.
            '`' if after.starts_with("```") => Some(true),
            '`' if after.len() < 3 && !forced => None,
            // A bullet does; a bare `-` or `*` at the start of a line is
            // ordinary text — `-3`, `*italic*` — and judged as such.
            '-' | '+' | '*' => match chars.next() {
                Some(c) if c.is_whitespace() => Some(true),
                None if !forced => None,
                _ => self.starts_a_sentence(after, forced),
            },
            // `1.` or `1)` and a space: a numbered item.
            d if d.is_ascii_digit() => {
                let digits = after.chars().take_while(char::is_ascii_digit).count();
                let rest = &after[digits..];
                match rest.chars().next() {
                    None if !forced => None,
                    Some('.' | ')') => match rest[1..].chars().next() {
                        None if !forced => None,
                        Some(c) if c.is_whitespace() => Some(true),
                        _ => self.starts_a_sentence(after, forced),
                    },
                    _ => self.starts_a_sentence(after, forced),
                }
            }
            _ => self.starts_a_sentence(after, forced),
        }
    }

    /// Whether `after` reads like the start of a sentence: its first character
    /// past any opening markup or quote is not a lowercase letter, and the
    /// line before did not end mid-clause. `None` when that character has not
    /// arrived.
    fn starts_a_sentence(&self, after: &str, forced: bool) -> Option<bool> {
        if matches!(self.last_char, Some(',' | ';')) {
            return Some(false);
        }
        let opening = |c: char| is_markup(c) || matches!(c, '"' | '\'' | '“' | '‘' | '(');
        match after.chars().find(|c| !opening(*c)) {
            Some(c) => Some(!c.is_lowercase()),
            None if forced => Some(true),
            None => None,
        }
    }

    /// Whether the next emitted character begins a line — true at the very
    /// start, and after a line break that ended a line. A break that merely
    /// wrapped a sentence is a space, and what follows a space is not a
    /// heading or a bullet.
    fn at_line_start(&self) -> bool {
        !self.emitted || self.pending == Pending::Break
    }

    /// Append `text`, honoring pending collapsed whitespace.
    fn push_text(&mut self, out: &mut String, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.emitted {
            match self.pending {
                Pending::Break => out.push('\n'),
                Pending::Space => out.push(' '),
                Pending::None => {}
            }
        }
        self.pending = Pending::None;
        self.emitted = true;
        out.push_str(text);
        self.last_char = text.chars().last();
    }

    /// Append a standalone notice on a line of its own: it stands in for a
    /// block, and a block ends where its lines do.
    fn push_word(&mut self, out: &mut String, word: &str) {
        self.pending = Pending::Break;
        self.push_text(out, word);
        self.pending = Pending::Break;
    }
}

/// One decision about the front of the held buffer.
enum Step {
    /// Emit this text, consuming `usize` bytes.
    Emit(String, usize),
    /// Drop `usize` bytes.
    Skip(usize),
    /// Collapse `usize` bytes of whitespace to one space.
    Space(usize),
    /// Collapse `usize` bytes of whitespace to a line break: the line ended.
    Break(usize),
    /// Drop `usize` bytes of heading marks, and remember the line is one.
    Heading(usize),
    /// Enter a code fence, consuming `usize` bytes.
    OpenFence(usize),
    /// Wait for more input.
    Hold,
}

/// Byte offset just past a fence's closing delimiter, if present.
fn find_fence_close(s: &str) -> Option<usize> {
    let idx = s.find("```")?;
    let after = idx + 3;
    // Consume the rest of the closing line so the fence leaves no residue.
    Some(s[after..].find('\n').map_or(s.len(), |i| after + i + 1))
}

/// Parse `[label](target)`, returning the label and bytes consumed.
fn parse_link(s: &str) -> Option<(String, usize)> {
    let close = s.find(']')?;
    let rest = &s[close + 1..];
    // Nothing after `]` yet. Streaming reaches here on every link, because the
    // `]` always arrives before the `(` — committing now would speak the label
    // *and* then the raw target as ordinary text.
    let next = rest.chars().next()?;
    if next != '(' {
        // `[text]` with no target — a reference link, or just brackets. Speak
        // the contents and drop the brackets.
        return Some((s[1..close].to_string(), close + 1));
    }
    let end = rest.find(')')?;
    Some((s[1..close].to_string(), close + 1 + end + 1))
}

/// Reduce a URL to its host: `https://example.com/a/b?c=d` → `example.com`.
///
/// A path read aloud is a string of slashes and hyphens nobody can act on; the
/// host is the part that carries meaning.
fn spoken_url(url: &str) -> String {
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let host = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(without_scheme);
    host.trim_end_matches(['.', ',']).to_string()
}

/// Whether `c` may begin a markup construct, so a word must stop before it.
fn is_markup(c: char) -> bool {
    matches!(c, '`' | '*' | '_' | '~' | '[')
}

/// Characters that carry no pronunciation and would otherwise be spoken as
/// their Unicode name or skipped inconsistently by the model.
fn is_unspeakable(c: char) -> bool {
    matches!(c, '|' | '•' | '‣' | '▪' | '●' | '◦')
        // Emoji and pictographs.
        || ('\u{1F300}'..='\u{1FAFF}').contains(&c)
        || ('\u{2600}'..='\u{27BF}').contains(&c)
        || ('\u{FE00}'..='\u{FE0F}').contains(&c)
        || c == '\u{200D}'
}

/// Normalize a complete string.
#[must_use]
pub fn normalize(text: &str) -> String {
    let mut n = Normalizer::new();
    let mut out = n.push(text);
    out.push_str(&n.finish());
    out.trim().to_string()
}

#[cfg(test)]
mod tests;
