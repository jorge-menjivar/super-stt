// SPDX-License-Identifier: GPL-3.0-only

//! Pure text helpers for the preview typer: normalization, the screen diff,
//! and stitching sliding-window previews into one transcript.
//!
//! These are keyboard- and session-state-free string algorithms shared by the
//! [`Typer`](crate::output::typer::Typer) state machine. Keeping them separate
//! makes them exhaustively unit-testable without a keyboard `Simulator`.

/// A character that must never reach the keyboard simulator. Backend
/// transcription output is untrusted (audit 2 Tier 3 #8): a non-whitespace C0/C1
/// control code (ESC, BEL, NUL, backspace, …) could drive a terminal escape
/// sequence, and a Unicode bidi override or zero-width char could visually spoof
/// or hide text in an editor. Ordinary whitespace (`\n`/`\r`/`\t`) is preserved
/// here and folded into single spaces by [`preprocess_text`]'s normalization.
pub(crate) fn is_unsafe_to_type(c: char) -> bool {
    (c.is_control() && !c.is_whitespace())
        // Bidi overrides + isolates (LRO/RLO/PDF, LRI/RLI/FSI/PDI).
        || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
        // Zero-width space/joiner/non-joiner + BOM.
        || matches!(c, '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}')
}

/// Strip every character [`is_unsafe_to_type`] flags. The single choke point
/// untrusted text must cross before it can reach the keyboard simulator
/// (audit 2 Tier 3 #8): both backend preview/final text (via
/// [`preprocess_text`]) and the fixed failure notices
/// ([`Typer::type_notice`](crate::output::typer::Typer::type_notice)) route
/// through this function rather than each re-implementing the filter, so the
/// property holds structurally instead of by convention.
#[must_use]
pub(crate) fn sanitize_for_typing(text: &str) -> String {
    text.chars().filter(|&c| !is_unsafe_to_type(c)).collect()
}

/// Normalize backend text: sanitize, drop a leading ellipsis, and fold runs of
/// whitespace into single spaces. This is what the typer stitches and diffs on.
///
/// No capitalization here. A sliding window starts wherever the capture was
/// cut, and a capital on its first word would land mid-sentence once the
/// window is stitched onto the one before it.
#[must_use]
pub(crate) fn normalize_text(text: &str) -> String {
    // Strip characters that must never be typed into the focused window before
    // any other processing (audit 2 Tier 3 #8).
    let sanitized = sanitize_for_typing(text);
    let text = sanitized.trim_start();
    let text = text.strip_prefix("...").unwrap_or(text);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Uppercase the first character. ASCII only, as the transcript always was.
#[must_use]
pub(crate) fn capitalize_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

/// Preprocess text - sanitize, normalize, remove ellipses, capitalize
#[must_use]
pub(crate) fn preprocess_text(text: &str, is_preview: bool) -> String {
    let mut text = capitalize_first(&normalize_text(text));

    // Add period for final output if it ends with alphanumeric
    if !is_preview && text.chars().last().is_some_and(char::is_alphanumeric) {
        text.push('.');
    }

    text
}

/// Find common prefix (in `char`s) between two strings
#[must_use]
pub(crate) fn find_common_prefix(text1: &str, text2: &str) -> usize {
    text1
        .chars()
        .zip(text2.chars())
        .take_while(|(c1, c2)| c1 == c2)
        .count()
}

/// How much of the two texts the overlap search covers, in chars: the tail of
/// the running transcript and the head of the new window.
const STITCH_SPAN: usize = 160;

/// How far from the end of the transcript, and from the start of the window,
/// the shared text may end and start, in chars. Consecutive windows share
/// their last and first few seconds of audio, so the text they share ends
/// where the transcript ends and starts where the window starts — give or
/// take a word the two windows heard differently at their cut points. A run
/// further in is a coincidence ("the cat" early in the transcript and again
/// at the front of a window), and stitching on a coincidence cuts the
/// transcript there.
const STITCH_SLACK: usize = 24;

/// The shortest run of identical text that counts as two windows having heard
/// the same speech. Shorter runs — a common word and its space — line up by
/// coincidence, and a false match drops the transcript's tail; a missed one
/// only repeats a word.
const MIN_STITCH_OVERLAP: usize = 6;

/// Where the tail of the running transcript and the head of a new window say
/// the same thing.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Overlap {
    /// Byte offset in the transcript just past the shared text.
    pub session_end: usize,
    /// Byte offset in the window just past the shared text.
    pub preview_end: usize,
}

/// Case-fold one char to one char, so indices keep lining up.
fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// The longest run of text the end of `session` and the start of `preview`
/// have in common, compared case-insensitively.
///
/// Only a run of at least [`MIN_STITCH_OVERLAP`] chars that ends within
/// [`STITCH_SLACK`] of the end of `session` and starts within [`STITCH_SLACK`]
/// of the start of `preview` counts. Among equally long runs the one starting
/// earliest in the window wins: the window's head is the shared audio.
///
/// The offsets are byte offsets, so slicing either text with them never lands
/// inside a multibyte char.
#[must_use]
pub(crate) fn find_overlap(session: &str, preview: &str) -> Option<Overlap> {
    // (byte start, byte end, folded char) for every char in each search span.
    let skip = session.chars().count().saturating_sub(STITCH_SPAN);
    let tail: Vec<(usize, usize, char)> = session
        .char_indices()
        .skip(skip)
        .map(|(b, c)| (b, b + c.len_utf8(), fold(c)))
        .collect();
    let head: Vec<(usize, usize, char)> = preview
        .char_indices()
        .take(STITCH_SPAN)
        .map(|(b, c)| (b, b + c.len_utf8(), fold(c)))
        .collect();
    let earliest_end = tail.len().saturating_sub(STITCH_SLACK);

    // Longest common substring over two rows: `cur[j + 1]` is the length of
    // the common run ending at `tail[i]` and `head[j]`.
    let mut prev = vec![0usize; head.len() + 1];
    let mut cur = vec![0usize; head.len() + 1];
    // (len, run start in `head`, end index in `tail`, end index in `head`)
    let mut best: Option<(usize, usize, usize, usize)> = None;
    for (i, &(_, _, sc)) in tail.iter().enumerate() {
        for (j, &(_, _, pc)) in head.iter().enumerate() {
            let len = if sc == pc { prev[j] + 1 } else { 0 };
            cur[j + 1] = len;
            let start = j + 1 - len;
            let qualifies = len >= MIN_STITCH_OVERLAP && i >= earliest_end && start < STITCH_SLACK;
            if qualifies
                && best.is_none_or(|(best_len, best_start, _, _)| {
                    len > best_len || (len == best_len && start < best_start)
                })
            {
                best = Some((len, start, i, j));
            }
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    best.map(|(_, _, i, j)| Overlap {
        session_end: tail[i].1,
        preview_end: head[j].1,
    })
}

/// Fold a sliding-window preview into the running transcript.
///
/// Early in a take the buffer is shorter than the window, so consecutive
/// windows are re-reads of the whole take and one extends the other. Once the
/// take outgrows the window, each window is only its last few seconds, and the
/// text it shares with the transcript ([`find_overlap`]) is where the
/// transcript continues from. With nothing shared, the window is appended
/// whole: a repeated word beats a preview that stops growing, and the final
/// pass replaces the preview anyway.
#[must_use]
pub(crate) fn merge_window_preview(session: &str, preview: &str) -> String {
    if session.is_empty() || preview.starts_with(session) {
        return preview.to_string();
    }
    if session.starts_with(preview) {
        return session.to_string();
    }
    if let Some(overlap) = find_overlap(session, preview) {
        let session_tail = &session[overlap.session_end..];
        let preview_tail = &preview[overlap.preview_end..];
        // The transcript never shrinks: a window that ends inside the shared
        // text has nothing new to add yet.
        if preview_tail.chars().count() >= session_tail.chars().count() {
            return format!("{}{preview_tail}", &session[..overlap.session_end]);
        }
        return session.to_string();
    }
    format!("{session} {preview}")
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;
