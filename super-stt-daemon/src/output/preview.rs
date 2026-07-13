// SPDX-License-Identifier: GPL-3.0-only

//! Pure text-diff helpers for the preview typer.
//!
//! These are keyboard- and session-state-free string algorithms shared by the
//! [`Typer`](crate::output::typer::Typer) state machine. Keeping them separate
//! makes them exhaustively unit-testable without a keyboard `Simulator`.

/// Preprocess text - normalize, remove ellipses, capitalize
#[must_use]
pub(crate) fn preprocess_text(text: &str, is_preview: bool) -> String {
    // Remove leading whitespaces
    let mut text = text.trim_start().to_string();

    // Remove starting ellipses if present
    if text.starts_with("...") {
        text = text[3..].to_string();
    }

    // Remove any leading whitespaces again after ellipses removal
    text = text.trim_start().to_string();

    // Normalize whitespace
    text = text.split_whitespace().collect::<Vec<_>>().join(" ");

    if text.is_empty() {
        return text;
    }

    // Uppercase the first letter
    let mut chars: Vec<char> = text.chars().collect();
    if let Some(first_char) = chars.first_mut() {
        *first_char = first_char.to_ascii_uppercase();
    }
    text = chars.iter().collect();

    // Add period for final output if it ends with alphanumeric
    if !is_preview && text.chars().last().is_some_and(char::is_alphanumeric) {
        text.push('.');
    }

    text
}

/// Simple extension check - much faster than complex word matching
#[must_use]
pub fn is_simple_extension(current: &str, new_text: &str) -> bool {
    if current.is_empty() {
        return !new_text.is_empty();
    }

    // Check if new text starts with current text
    new_text.starts_with(current) && new_text.len() > current.len()
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

/// Find where the last `length_of_match` characters of `text1` match a
/// substring of `text2`, returning `Some(byte_offset)` in `text2` just past the
/// rightmost such match (so callers can `&text2[pos..]` safely), or `None` if
/// there is no match.
///
/// The offset is a byte index — not a char index — so slicing `text2` with it
/// never lands inside a multibyte UTF-8 sequence.
pub(crate) fn find_tail_match_in_text(
    text1: &str,
    text2: &str,
    length_of_match: usize,
) -> Option<usize> {
    let text1_chars: Vec<char> = text1.chars().collect();
    let text2_chars: Vec<char> = text2.chars().collect();

    // Either side too short to hold a `length_of_match` window.
    if text1_chars.len() < length_of_match || text2_chars.len() < length_of_match {
        return None;
    }

    // The trailing `length_of_match` chars of text1 we want to locate in text2.
    let target = &text1_chars[text1_chars.len() - length_of_match..];

    // Scan text2 right-to-left for the last occurrence of `target`. Compare the
    // char slices directly rather than allocating a `Vec` per position.
    for i in 0..=(text2_chars.len() - length_of_match) {
        let end_char = text2_chars.len() - i;
        let start_char = end_char - length_of_match;
        if text2_chars[start_char..end_char] == *target {
            // `end_char` is a CHAR index into text2; map it to a byte offset so
            // callers can byte-slice `&text2[pos..]` without splitting a
            // multibyte char. No chars past the match => the byte offset is
            // `text2.len()`.
            return Some(
                text2
                    .char_indices()
                    .nth(end_char)
                    .map_or(text2.len(), |(b, _)| b),
            );
        }
    }

    None
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;
