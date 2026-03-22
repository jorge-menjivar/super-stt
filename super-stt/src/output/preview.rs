// SPDX-License-Identifier: GPL-3.0-only

use crate::output::keyboard::Simulator;
use log::{debug, info, warn};

/// State for tracking preview updates
pub struct State {
    pub last_transcription: String,
    pub prev_text: String,
    /// Complete transcription built from all audio (for final output)
    pub full_session_text: String,
    /// When we last saw substantial text growth (to commit to full session)
    pub last_growth_time: std::time::Instant,
    /// History of transcriptions for stabilization
    pub text_storage: Vec<String>,
    /// Text confirmed by appearing in multiple transcriptions
    pub stabilized_text: String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            last_transcription: String::new(),
            prev_text: String::new(),
            full_session_text: String::new(),
            last_growth_time: std::time::Instant::now(),
            text_storage: Vec::new(),
            stabilized_text: String::new(),
        }
    }
}

/// Unified, simplified preview typer that combines the best of both approaches
pub struct Typer {
    keyboard_simulator: Simulator,
    state: State,
}

impl Typer {
    #[must_use]
    pub fn new(keyboard_simulator: Simulator) -> Self {
        Self {
            keyboard_simulator,
            state: State::default(),
        }
    }

    #[must_use]
    pub fn write_method_name(&self) -> &'static str {
        self.keyboard_simulator.name()
    }

    /// Extract the simulator so it can be cached for reuse.
    #[must_use]
    pub fn take_simulator(self) -> Simulator {
        self.keyboard_simulator
    }
}

impl Typer {
    /// Preprocess text - normalize, remove ellipses, capitalize
    #[must_use]
    pub fn preprocess_text(text: &str, is_preview: bool) -> String {
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

    /// Find common prefix between two strings
    #[must_use]
    fn find_common_prefix(text1: &str, text2: &str) -> usize {
        text1
            .chars()
            .zip(text2.chars())
            .take_while(|(c1, c2)| c1 == c2)
            .count()
    }

    /// Apply a simple differential update by backspacing and retyping from first difference
    pub fn apply_simple_diff(&mut self, old_text: &str, new_text: &str) -> usize {
        // Safety checks
        if old_text == new_text {
            return 0;
        }

        if old_text.is_empty() && !new_text.is_empty() {
            let _ = self.keyboard_simulator.type_text(new_text);
            debug!("Failed to type new text");
            return new_text.len();
        }

        if new_text.is_empty() {
            // Skip
            return 0;
        }

        let old_chars: Vec<char> = old_text.chars().collect();
        let new_chars: Vec<char> = new_text.chars().collect();

        // Find first different character position
        let common_prefix = Self::find_common_prefix(old_text, new_text);

        // Calculate what to delete and what to type
        let chars_to_delete = old_chars.len() - common_prefix;
        let text_to_type: String = new_chars[common_prefix..].iter().collect();

        debug!(
            "Simple diff: prefix={}, delete={}, type='{}'",
            common_prefix,
            chars_to_delete,
            text_to_type.chars().take(20).collect::<String>()
        );

        // Backspace to the first different position
        let _ = self.keyboard_simulator.backspace_n(chars_to_delete);

        // Type the new part
        let _ = self.keyboard_simulator.type_text(&text_to_type);

        text_to_type.len()
    }

    /// Update preview text using two-phase approach
    pub fn update_preview(&mut self, new_text: &str, actually_typed: &mut String) {
        let processed_text = Self::preprocess_text(new_text, true);

        info!(
            "Preview update: new='{}', prev='{}', typed='{}'",
            processed_text.chars().take(30).collect::<String>(),
            self.state.prev_text.chars().take(30).collect::<String>(),
            actually_typed.chars().take(30).collect::<String>()
        );

        // Skip if text hasn't changed
        if processed_text == self.state.prev_text {
            debug!("Text unchanged, skipping");
            return;
        }

        // Skip empty text
        if processed_text.is_empty() {
            debug!("Empty text, skipping");
            return;
        }

        // PHASE 1: Stabilization and session text update
        self.update_with_stabilization(&processed_text);

        // PHASE 2: Decide what to show on screen
        let display_text = self.build_display_text(&processed_text);

        info!(
            "Display logic: display='{}', session='{}', stabilized='{}'",
            display_text.chars().take(30).collect::<String>(),
            self.state
                .full_session_text
                .chars()
                .take(30)
                .collect::<String>(),
            self.state
                .stabilized_text
                .chars()
                .take(30)
                .collect::<String>()
        );

        // Apply the update to screen
        self.apply_text_update(&display_text, actually_typed);
        self.state.prev_text = processed_text;
    }

    /// Stabilization and session text update (Phase 1)
    fn update_with_stabilization(&mut self, new_preview_text: &str) {
        // Add current text to storage
        self.state.text_storage.push(new_preview_text.to_string());

        // Keep only recent texts for stabilization (prevent unbounded growth)
        if self.state.text_storage.len() > 10 {
            self.state.text_storage.remove(0);
        }

        // Find common prefix between last two texts
        if self.state.text_storage.len() >= 2 {
            let last_two = &self.state.text_storage[self.state.text_storage.len() - 2..];
            let common_prefix = Self::find_common_prefix(&last_two[0], &last_two[1]);
            let prefix_text = last_two[0].chars().take(common_prefix).collect::<String>();

            // Only update stabilized text if we found a longer stable prefix
            if prefix_text.len() > self.state.stabilized_text.len() {
                self.state.stabilized_text = prefix_text;
                debug!(
                    "Updated stabilized text: '{}'",
                    self.state
                        .stabilized_text
                        .chars()
                        .take(30)
                        .collect::<String>()
                );
            }
        }

        // Update full session text using stabilized text + tail matching
        self.update_full_session_text(new_preview_text);
    }

    /// Update the full session text using stabilized text as base
    #[allow(clippy::cast_sign_loss)]
    fn update_full_session_text(&mut self, new_preview_text: &str) {
        // If we have stabilized text, use it as our base
        if !self.state.stabilized_text.is_empty()
            && self.state.stabilized_text.len() > self.state.full_session_text.len()
        {
            self.state.full_session_text = self.state.stabilized_text.clone();
            self.state.last_growth_time = std::time::Instant::now();
            debug!(
                "Updated session from stabilized: '{}'",
                self.state
                    .full_session_text
                    .chars()
                    .take(30)
                    .collect::<String>()
            );
        }

        // Only grow the session text, never shrink it
        if self.state.full_session_text.is_empty() {
            self.state.full_session_text = new_preview_text.to_string();
            self.state.last_growth_time = std::time::Instant::now();
            debug!(
                "Started session text: '{}'",
                self.state
                    .full_session_text
                    .chars()
                    .take(30)
                    .collect::<String>()
            );
            return;
        }

        // Check if preview text extends our session text
        if new_preview_text.len() > self.state.full_session_text.len()
            && new_preview_text.starts_with(&self.state.full_session_text)
        {
            // Perfect extension - just grow
            self.state.full_session_text = new_preview_text.to_string();
            self.state.last_growth_time = std::time::Instant::now();
            debug!(
                "Extended session text to: '{}'",
                self.state
                    .full_session_text
                    .chars()
                    .take(40)
                    .collect::<String>()
            );
            return;
        }

        // Use tail matching to extend session with new content
        let matching_pos =
            Self::find_tail_match_in_text(&self.state.full_session_text, new_preview_text, 3);
        if matching_pos >= 0 {
            let extended = format!(
                "{}{}",
                self.state.full_session_text,
                &new_preview_text[matching_pos as usize..]
            );
            if extended.len() > self.state.full_session_text.len() {
                self.state.full_session_text = extended;
                self.state.last_growth_time = std::time::Instant::now();
                debug!(
                    "Extended session via tail match: '{}'",
                    self.state
                        .full_session_text
                        .chars()
                        .take(40)
                        .collect::<String>()
                );
            }
        }
    }

    /// Build the display text (Phase 2) - what actually shows on screen
    #[allow(clippy::cast_sign_loss)]
    fn build_display_text(&mut self, preview_text: &str) -> String {
        // Use stabilized text as base, but be smart about it

        // If no stabilized text yet, show the preview
        if self.state.stabilized_text.is_empty() {
            return preview_text.to_string();
        }

        // Try tail matching first
        let matching_pos =
            Self::find_tail_match_in_text(&self.state.stabilized_text, preview_text, 3);

        if matching_pos >= 0 {
            // Found overlap - combine stabilized text with new part from preview
            let combined = format!(
                "{}{}",
                self.state.stabilized_text,
                &preview_text[matching_pos as usize..]
            );
            return combined;
        }

        // No tail match found - be conservative to avoid text loss
        // Prefer the longer text (session text or preview) to avoid disappearing words
        let best_text = if self.state.full_session_text.len() >= preview_text.len() {
            &self.state.full_session_text
        } else {
            preview_text
        };

        best_text.to_string()
    }

    /// Find the position where the last 'n' characters of text1 match with a substring in text2.
    fn find_tail_match_in_text(text1: &str, text2: &str, length_of_match: usize) -> i32 {
        // Check if either text is too short
        if text1.chars().count() < length_of_match || text2.chars().count() < length_of_match {
            return -1;
        }

        let text1_chars: Vec<char> = text1.chars().collect();
        let text2_chars: Vec<char> = text2.chars().collect();

        // The end portion of text1 that we want to find in text2
        let target_substring: Vec<char> =
            text1_chars[text1_chars.len() - length_of_match..].to_vec();

        // Loop through text2 from right to left
        for i in 0..=(text2_chars.len() - length_of_match) {
            let start_pos = text2_chars.len() - i - length_of_match;
            let end_pos = text2_chars.len() - i;
            let current_substring: Vec<char> = text2_chars[start_pos..end_pos].to_vec();

            // Compare substrings
            if current_substring == target_substring {
                return i32::try_from(end_pos).unwrap_or_default();
            }
        }

        -1
    }

    /// Process final text (completed sentence) - Uses full session audio
    pub fn process_final_text(&mut self, transcription_result: &str) {
        // No preview typing, type directly
        let processed_text =
            crate::output::preview::Typer::preprocess_text(transcription_result, false);
        let final_text = format!("{processed_text} ");
        if let Err(e) = self.keyboard_simulator.type_text(&final_text) {
            warn!("Failed to type final transcription: {e}");
        } else {
            info!("Step 6 complete: Final transcription typed directly");
        }

        // Reset state for next sentence - but keep the full session text for user reference
        self.state.prev_text.clear();
        self.state.last_transcription = processed_text;
        self.state.last_growth_time = std::time::Instant::now();

        info!(
            "Completed sentence. Session text: '{}'",
            self.state
                .full_session_text
                .chars()
                .take(50)
                .collect::<String>()
        );

        // Clear session for next recording
        self.state.full_session_text.clear();
    }

    /// Apply text update to screen (common logic)
    fn apply_text_update(&mut self, new_text: &str, actually_typed: &mut String) {
        let old_char_count = actually_typed.chars().count();
        let new_char_count = new_text.chars().count();

        info!(
            "Typing logic: old_typed='{}', new_display='{}', old_count={}, new_count={}",
            actually_typed.chars().take(30).collect::<String>(),
            new_text.chars().take(30).collect::<String>(),
            old_char_count,
            new_char_count
        );

        let actual_chars_on_screen;
        // Screen is empty, just type the new text
        if old_char_count == 0 {
            info!(
                "Screen empty, typing new text: '{}'",
                new_text.chars().take(30).collect::<String>()
            );
            let _ = self.keyboard_simulator.type_text(&format!("{new_text} "));
            actual_chars_on_screen = new_char_count;
        }
        // Screen is not empty, check if new text starts with actually typed text
        else if new_text.starts_with(actually_typed.as_str())
            && new_text.len() > actually_typed.len()
        {
            // Perfect extension - just add the suffix
            let suffix = &new_text[actually_typed.len()..];
            info!("Perfect extension, adding suffix: '{suffix}'");
            let _ = self.keyboard_simulator.type_text(&format!("{suffix} "));
            actual_chars_on_screen = new_char_count;
        } else {
            // Need to replace - use differential update
            info!("Need replacement, using diff update");
            let net_change = self.apply_simple_diff(actually_typed, new_text);

            // Calculate actual characters on screen based on the net change
            let new_count = old_char_count + net_change;
            actual_chars_on_screen = new_count.max(0);

            info!(
                "Replaced: {}{} chars (screen total: {})",
                if net_change > 0 { "+" } else { "" },
                net_change,
                actual_chars_on_screen
            );
        }

        // Update state to match what we think is actually on screen
        if actual_chars_on_screen == new_char_count {
            // Typing succeeded completely
            actually_typed.clear();
            actually_typed.push_str(new_text);
            info!(
                "Updated actually_typed to: '{}'",
                actually_typed.chars().take(30).collect::<String>()
            );
        } else {
            // Typing failed or was partial - but for preview, we should still track what we attempted to type
            // This ensures preview clearing works correctly even when there are typing discrepancies
            warn!(
                "Character count mismatch: expected {new_char_count}, actual {actual_chars_on_screen}, updating actually_typed anyway"
            );
            actually_typed.clear();
            actually_typed.push_str(new_text);
            info!(
                "Force updated actually_typed to: '{}'",
                actually_typed.chars().take(30).collect::<String>()
            );
        }
    }

    /// Clear all typed text and reset state
    pub fn clear_preview(&mut self, actually_typed: &mut String) {
        info!("clear_preview called with actually_typed: '{actually_typed}'");

        if actually_typed.is_empty() {
            info!("actually_typed is empty, nothing to clear");
            return;
        }

        let chars_to_delete = actually_typed.chars().count();
        info!("Backspacing {chars_to_delete} characters");

        if let Err(e) = self.keyboard_simulator.backspace_n(chars_to_delete) {
            warn!("Failed to backspace preview text: {e}");
        } else {
            info!("Successfully backspaced {chars_to_delete} characters");
        }

        actually_typed.clear();

        // Also clear state when explicitly clearing preview
        self.state.prev_text.clear();
        self.state.last_transcription.clear();
        self.state.full_session_text.clear();
        self.state.last_growth_time = std::time::Instant::now();

        info!("Cleared all {chars_to_delete} characters and reset state");
    }

    /// In a single input session, backspace preview chars then type final text
    ///
    /// # Errors
    /// This function can fail if the enigo initialization fails or if the text typing task fails.
    pub fn replace_preview_and_type(&mut self, preview_chars: usize, text: &str) {
        // Use unified preprocessor for final text (adds period, capitalizes)
        let processed_text = Typer::preprocess_text(text, false);
        let text_to_type = processed_text + " ";

        // Erase preview in batches
        if preview_chars > 0 {
            let _ = self.keyboard_simulator.backspace_n(preview_chars);
        }

        let _ = self.keyboard_simulator.type_text(&text_to_type);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preprocess_text() {
        // Basic functionality
        assert_eq!(Typer::preprocess_text("hello world", true), "Hello world");
        assert_eq!(Typer::preprocess_text("hello world", false), "Hello world.");
        assert_eq!(Typer::preprocess_text("", true), "");

        assert_eq!(
            Typer::preprocess_text("...hello world", true),
            "Hello world"
        );
        assert_eq!(
            Typer::preprocess_text("  ...  hello world  ", true),
            "Hello world"
        );
        assert_eq!(
            Typer::preprocess_text("  multiple   spaces  ", true),
            "Multiple spaces"
        );
    }

    #[test]
    fn test_is_simple_extension() {
        assert!(Typer::is_simple_extension("hello", "hello world"));
        assert!(Typer::is_simple_extension("", "hello"));
        assert!(!Typer::is_simple_extension("hello", "hi world"));
        assert!(!Typer::is_simple_extension("hello", "hello"));
        assert!(!Typer::is_simple_extension("hello world", "hello"));
    }

    #[test]
    fn test_find_tail_match_in_text() {
        // Test the key case: "engi" should match with "engineer"
        assert_eq!(
            Typer::find_tail_match_in_text("hello engi", "engineer is good", 4),
            4
        );

        // Test basic tail matching
        assert_eq!(
            Typer::find_tail_match_in_text("hello world", "world is nice", 5),
            5
        );

        // Test no match
        assert_eq!(Typer::find_tail_match_in_text("hello", "goodbye", 3), -1);

        // Test short strings
        assert_eq!(Typer::find_tail_match_in_text("hi", "hello", 3), -1);

        // Test exact match at end
        assert_eq!(Typer::find_tail_match_in_text("abc", "xyzabc", 3), 6);
    }

    #[test]
    fn test_find_common_prefix() {
        assert_eq!(Typer::find_common_prefix("hello world", "hello there"), 6);
        assert_eq!(Typer::find_common_prefix("abc", "def"), 0);
        assert_eq!(Typer::find_common_prefix("same text", "same text"), 9);
    }
}
