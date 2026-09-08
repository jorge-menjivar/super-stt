// SPDX-License-Identifier: GPL-3.0-only

//! The preview typer state machine: session/stabilization state plus the
//! keyboard-driving update logic. The pure text-diff algorithms it builds on
//! live in [`crate::output::preview`].

use crate::output::keyboard::Simulator;
use crate::output::preview::{
    find_common_prefix, find_tail_match_in_text, preprocess_text, sanitize_for_typing,
};
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

/// How long [`Typer::type_notice`] waits before typing, so the user's shortcut
/// keys can be fully released first.
///
/// A failure notice can be emitted within milliseconds of a keypress, at which
/// point the shortcut's modifier keys (Ctrl/Alt/Super/Shift) are usually still
/// physically held down. Simulated keystrokes sent during that window arrive
/// *modified*, so the focused application interprets the notice as shortcuts
/// rather than text.
///
/// Two distinct cases hit this window, which is why the wait applies to every
/// notice rather than just the first:
///
/// - The no-model preflight rejects the request before capture even starts, so
///   the notice follows the *start* keypress almost immediately.
/// - In manual stop mode the user presses the hotkey to end the recording, and
///   a failure that surfaces quickly after that — a model unloaded mid-cycle,
///   say — puts the notice right behind the *stop* keypress.
///
/// This does not apply to transcription output: that is typed after speech has
/// been captured and transcribed, long past any plausible key-release window.
const NOTICE_KEY_RELEASE_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

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

    /// Drive the screen from `old_text` to `new_text`: backspace to the first
    /// differing character and type the rest.
    ///
    /// The one path every preview update takes. An empty screen (common prefix
    /// 0, nothing to delete) and a pure extension (the prefix is all of
    /// `old_text`, nothing to delete) are just what the diff computes, not
    /// separate branches that can disagree about what they typed.
    async fn retype_from_first_difference(&mut self, old_text: &str, new_text: &str) {
        if old_text == new_text {
            return;
        }

        let common_prefix = find_common_prefix(old_text, new_text);
        let chars_to_delete = old_text.chars().count() - common_prefix;
        let text_to_type: String = new_text.chars().skip(common_prefix).collect();

        debug!(
            "Retype: prefix={common_prefix}, delete={chars_to_delete}, type='{}'",
            text_to_type.chars().take(20).collect::<String>()
        );

        if chars_to_delete > 0
            && let Err(e) = self.keyboard_simulator.backspace_n(chars_to_delete).await
        {
            debug!("Failed to backspace preview text: {e}");
        }
        if !text_to_type.is_empty()
            && let Err(e) = self.keyboard_simulator.type_text(&text_to_type).await
        {
            debug!("Failed to type preview text: {e}");
        }
    }

    /// Update preview text using two-phase approach
    pub async fn update_preview(&mut self, new_text: &str, actually_typed: &mut String) {
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
        self.apply_text_update(&display_text, actually_typed).await;
        self.state.prev_text = processed_text;
    }

    /// Process final text (completed sentence) - Uses full session audio
    pub async fn process_final_text(&mut self, transcription_result: &str) {
        // No preview typing, type directly
        let processed_text = preprocess_text(transcription_result, false);

        // An empty transcript has nothing to type. Without this guard the
        // `format!("{processed_text} ")` below deposits a bare space into the
        // user's focused window every time a recording produces no text.
        if processed_text.trim().is_empty() {
            info!("Final transcription is empty; typing nothing");
            self.reset_after_recording(processed_text);
            return;
        }

        let final_text = format!("{processed_text} ");
        if let Err(e) = self.keyboard_simulator.type_text(&final_text).await {
            warn!("Failed to type final transcription: {e}");
        } else {
            info!("Step 6 complete: Final transcription typed directly");
        }

        self.reset_after_recording(processed_text);
    }

    /// Clear the per-recording transcript state so the next recording starts
    /// clean. Preview tail-matching reads `full_session_text` and `prev_text`,
    /// so anything left here would be treated as a prefix to extend.
    ///
    /// Split out of [`Self::process_final_text`] because the no-speech path
    /// finishes a recording without typing anything and still has to reset.
    pub fn reset_after_recording(&mut self, last_transcription: String) {
        self.state.prev_text.clear();
        self.state.last_transcription = last_transcription;
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

    /// Type a fixed daemon-authored notice into the focused window.
    ///
    /// Deliberately **not** `process_final_text`. That method mutates
    /// transcript state (`last_transcription`, `prev_text`, `full_session_text`)
    /// which feeds preview tail-matching on the next recording, and applies
    /// transcript semantics — capitalization, a trailing period, a trailing
    /// space — that a fixed marker must not inherit. A notice is typed verbatim
    /// and leaves session state alone.
    ///
    /// Routed through the same [`sanitize_for_typing`] choke point as every
    /// other write path (audit 2 Tier 3 #8). The callers pass constants, so
    /// this is a no-op today; it is here so the property holds by
    /// construction.
    ///
    /// Waits [`NOTICE_KEY_RELEASE_DELAY`] before typing — see that constant for
    /// why.
    pub async fn type_notice(&mut self, notice: &str) {
        // Let the user's hotkey come back up first. The no-model preflight
        // rejects before capture starts, so this can run within milliseconds of
        // the press, while the shortcut's modifiers are still physically held.
        // Typing then delivers *modified* keystrokes to the focused window —
        // the notice would fire shortcuts in the user's application instead of
        // inserting text.
        tokio::time::sleep(NOTICE_KEY_RELEASE_DELAY).await;

        let sanitized = sanitize_for_typing(notice);
        if let Err(e) = self.keyboard_simulator.type_text(&sanitized).await {
            warn!("Failed to type notice: {e}");
        } else {
            info!("Typed failure notice: {sanitized}");
        }
    }

    /// Put `new_text` on screen in place of what `actually_typed` says is
    /// there, then record `new_text` as what is there now.
    ///
    /// The mirror has to be exactly the screen: `clear_preview` backspaces its
    /// length and the next update diffs against it. It used to drift by one
    /// space per update — the first text and every extension were typed with a
    /// trailing space the mirror never held — so extensions doubled spaces,
    /// replacements backspaced too few characters and left fragments of the
    /// old text behind, and the clear at the end of a recording left the first
    /// characters of the preview in front of the final transcript.
    async fn apply_text_update(&mut self, new_text: &str, actually_typed: &mut String) {
        info!(
            "Typing logic: old_typed='{}', new_display='{}'",
            actually_typed.chars().take(30).collect::<String>(),
            new_text.chars().take(30).collect::<String>(),
        );

        self.retype_from_first_difference(actually_typed, new_text)
            .await;

        actually_typed.clear();
        actually_typed.push_str(new_text);
    }

    /// Clear all typed text and reset state
    pub async fn clear_preview(&mut self, actually_typed: &mut String) {
        info!("clear_preview called with actually_typed: '{actually_typed}'");

        if actually_typed.is_empty() {
            info!("actually_typed is empty, nothing to clear");
            return;
        }

        let chars_to_delete = actually_typed.chars().count();
        info!("Backspacing {chars_to_delete} characters");

        if let Err(e) = self.keyboard_simulator.backspace_n(chars_to_delete).await {
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
