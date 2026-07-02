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
/// substring of `text2`, returning the **byte offset** in `text2` just past
/// that match (so callers can `&text2[pos..]` safely), or `-1` if no match.
///
/// The offset is a byte index — not a char index — so slicing `text2` with
/// it never lands inside a multibyte UTF-8 sequence.
pub(crate) fn find_tail_match_in_text(text1: &str, text2: &str, length_of_match: usize) -> i32 {
    // Check if either text is too short
    if text1.chars().count() < length_of_match || text2.chars().count() < length_of_match {
        return -1;
    }

    let text1_chars: Vec<char> = text1.chars().collect();
    let text2_chars: Vec<char> = text2.chars().collect();

    // The end portion of text1 that we want to find in text2
    let target_substring: Vec<char> = text1_chars[text1_chars.len() - length_of_match..].to_vec();

    // Loop through text2 from right to left
    for i in 0..=(text2_chars.len() - length_of_match) {
        let start_pos = text2_chars.len() - i - length_of_match;
        let end_pos = text2_chars.len() - i;
        let current_substring: Vec<char> = text2_chars[start_pos..end_pos].to_vec();

        // Compare substrings
        if current_substring == target_substring {
            // `end_pos` is a CHAR index into text2; map it to a byte offset
            // so callers can byte-slice `&text2[pos..]` without splitting a
            // multibyte char. `nth(end_pos) == None` means the match ends at
            // the very end of text2, i.e. byte offset `text2.len()`.
            let byte_off = text2
                .char_indices()
                .nth(end_pos)
                .map_or(text2.len(), |(b, _)| b);
            return i32::try_from(byte_off).unwrap_or(i32::MAX);
        }
    }

    -1
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;
