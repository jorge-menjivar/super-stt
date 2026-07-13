// SPDX-License-Identifier: GPL-3.0-only

//! The preview typer state machine: session/stabilization state plus the
//! keyboard-driving update logic. The pure text-diff algorithms it builds on
//! live in [`crate::output::preview`].

use crate::output::keyboard::Simulator;
use crate::output::preview::{find_common_prefix, find_tail_match_in_text, preprocess_text};
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

impl State {
    /// Stabilization and session text update (Phase 1).
    ///
    /// Keyboard-free: mutates only session/stabilization state, so it is
    /// independently testable.
    fn update_with_stabilization(&mut self, new_preview_text: &str) {
        // Add current text to storage
        self.text_storage.push(new_preview_text.to_string());

        // Keep only recent texts for stabilization (prevent unbounded growth)
        if self.text_storage.len() > 10 {
            self.text_storage.remove(0);
        }

        // Find common prefix between last two texts
        if self.text_storage.len() >= 2 {
            let last_two = &self.text_storage[self.text_storage.len() - 2..];
            let common_prefix = find_common_prefix(&last_two[0], &last_two[1]);
            let prefix_text = last_two[0].chars().take(common_prefix).collect::<String>();

            // Only update stabilized text if we found a longer stable prefix
            if prefix_text.len() > self.stabilized_text.len() {
                self.stabilized_text = prefix_text;
                debug!(
                    "Updated stabilized text: '{}'",
                    self.stabilized_text.chars().take(30).collect::<String>()
                );
            }
        }

        // Update full session text using stabilized text + tail matching
        self.update_full_session_text(new_preview_text);
    }

    /// Update the full session text using stabilized text as base.
    fn update_full_session_text(&mut self, new_preview_text: &str) {
        // If we have stabilized text, use it as our base
        if !self.stabilized_text.is_empty()
            && self.stabilized_text.len() > self.full_session_text.len()
        {
            self.full_session_text = self.stabilized_text.clone();
            self.last_growth_time = std::time::Instant::now();
            debug!(
                "Updated session from stabilized: '{}'",
                self.full_session_text.chars().take(30).collect::<String>()
            );
        }

        // Only grow the session text, never shrink it
        if self.full_session_text.is_empty() {
            self.full_session_text = new_preview_text.to_string();
            self.last_growth_time = std::time::Instant::now();
            debug!(
                "Started session text: '{}'",
                self.full_session_text.chars().take(30).collect::<String>()
            );
            return;
        }

        // Check if preview text extends our session text
        if new_preview_text.len() > self.full_session_text.len()
            && new_preview_text.starts_with(&self.full_session_text)
        {
            // Perfect extension - just grow
            self.full_session_text = new_preview_text.to_string();
            self.last_growth_time = std::time::Instant::now();
            debug!(
                "Extended session text to: '{}'",
                self.full_session_text.chars().take(40).collect::<String>()
            );
            return;
        }

        // Use tail matching to extend session with new content
        if let Some(pos) = find_tail_match_in_text(&self.full_session_text, new_preview_text, 3) {
            let extended = format!("{}{}", self.full_session_text, &new_preview_text[pos..]);
            if extended.len() > self.full_session_text.len() {
                self.full_session_text = extended;
                self.last_growth_time = std::time::Instant::now();
                debug!(
                    "Extended session via tail match: '{}'",
                    self.full_session_text.chars().take(40).collect::<String>()
                );
            }
        }
    }

    /// Build the display text (Phase 2) - what actually shows on screen.
    fn build_display_text(&self, preview_text: &str) -> String {
        // Use stabilized text as base, but be smart about it

        // If no stabilized text yet, show the preview
        if self.stabilized_text.is_empty() {
            return preview_text.to_string();
        }

        // Try tail matching first
        if let Some(pos) = find_tail_match_in_text(&self.stabilized_text, preview_text, 3) {
            // Found overlap - combine stabilized text with new part from preview
            return format!("{}{}", self.stabilized_text, &preview_text[pos..]);
        }

        // No tail match found - be conservative to avoid text loss
        // Prefer the longer text (session text or preview) to avoid disappearing words
        let best_text = if self.full_session_text.len() >= preview_text.len() {
            &self.full_session_text
        } else {
            preview_text
        };

        best_text.to_string()
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

    /// Apply a simple differential update by backspacing to the first differing
    /// character and retyping the rest. Returns the **net change in screen
    /// characters** (chars typed minus chars deleted) so callers accounting in
    /// chars stay consistent — mixing this with a byte length would drift on any
    /// multibyte text.
    pub fn apply_simple_diff(&mut self, old_text: &str, new_text: &str) -> isize {
        // Safety checks
        if old_text == new_text {
            return 0;
        }

        if old_text.is_empty() && !new_text.is_empty() {
            if let Err(e) = self.keyboard_simulator.type_text(new_text) {
                debug!("Failed to type new text: {e}");
            }
            return isize::try_from(new_text.chars().count()).unwrap_or(isize::MAX);
        }

        if new_text.is_empty() {
            // Skip
            return 0;
        }

        let old_chars: Vec<char> = old_text.chars().collect();
        let new_chars: Vec<char> = new_text.chars().collect();

        // Find first different character position
        let common_prefix = find_common_prefix(old_text, new_text);

        // Calculate what to delete and what to type
        let chars_to_delete = old_chars.len() - common_prefix;
        let text_to_type: String = new_chars[common_prefix..].iter().collect();
        let chars_to_type = new_chars.len() - common_prefix;

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

        // Net screen delta in chars: what we added minus what we removed.
        isize::try_from(chars_to_type).unwrap_or(isize::MAX)
            - isize::try_from(chars_to_delete).unwrap_or(isize::MAX)
    }

    /// Update preview text using two-phase approach
    pub fn update_preview(&mut self, new_text: &str, actually_typed: &mut String) {
        let processed_text = preprocess_text(new_text, true);

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
        self.state.update_with_stabilization(&processed_text);

        // PHASE 2: Decide what to show on screen
        let display_text = self.state.build_display_text(&processed_text);

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

    /// Process final text (completed sentence) - Uses full session audio
    pub fn process_final_text(&mut self, transcription_result: &str) {
        // No preview typing, type directly
        let processed_text = preprocess_text(transcription_result, false);
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
        info!(
            "Typing logic: old_typed='{}', new_display='{}'",
            actually_typed.chars().take(30).collect::<String>(),
            new_text.chars().take(30).collect::<String>(),
        );

        if actually_typed.is_empty() {
            // Screen is empty — type the whole thing.
            info!(
                "Screen empty, typing new text: '{}'",
                new_text.chars().take(30).collect::<String>()
            );
            let _ = self.keyboard_simulator.type_text(&format!("{new_text} "));
        } else if new_text.starts_with(actually_typed.as_str())
            && new_text.len() > actually_typed.len()
        {
            // Perfect extension — append only the new suffix.
            let suffix = &new_text[actually_typed.len()..];
            info!("Perfect extension, adding suffix: '{suffix}'");
            let _ = self.keyboard_simulator.type_text(&format!("{suffix} "));
        } else {
            // Replacement — backspace to the first difference and retype.
            let net_change = self.apply_simple_diff(actually_typed, new_text);
            info!("Diff replacement: net {net_change} char(s)");
        }

        // `actually_typed` mirrors what we drove onto the screen so
        // `clear_preview` backspaces the right count next time. Every branch
        // above leaves the screen showing `new_text`. The keyboard results are
        // best-effort and unchecked, so there is no measured count to reconcile
        // against — the old byte-vs-char reconciliation was both wrong (it added
        // `apply_simple_diff`'s byte length to a char count) and dead (both of
        // its branches did exactly this assignment).
        actually_typed.clear();
        actually_typed.push_str(new_text);
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
}

#[cfg(test)]
#[path = "typer_tests.rs"]
mod tests;
