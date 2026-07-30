// SPDX-License-Identifier: GPL-3.0-only
use super::*;
use crate::output::keyboard::Simulator;
use crate::output::notice;

// ---------------------------------------------------------------------------
// State machine characterization (keyboard-free transitions)
// ---------------------------------------------------------------------------

#[test]
fn build_display_text_returns_preview_when_no_stabilized_text() {
    let s = State::default();
    assert_eq!(s.build_display_text("Hello world"), "Hello world");
}

#[test]
fn build_display_text_combines_stabilized_with_tail_matched_suffix() {
    let s = State {
        stabilized_text: "Hello engi".to_string(),
        ..Default::default()
    };
    // "engi"'s tail "ngi" overlaps "engineer…"; suffix "neer is good" is grafted on.
    assert_eq!(
        s.build_display_text("engineer is good"),
        "Hello engineer is good"
    );
}

#[test]
fn build_display_text_prefers_session_when_no_tail_match_and_session_longer() {
    let s = State {
        stabilized_text: "abc".to_string(),
        full_session_text: "abcdefghij".to_string(),
        ..Default::default()
    };
    assert_eq!(s.build_display_text("wxyz"), "abcdefghij");
}

#[test]
fn build_display_text_prefers_preview_when_no_tail_match_and_preview_longer() {
    let s = State {
        stabilized_text: "abc".to_string(),
        full_session_text: "ab".to_string(),
        ..Default::default()
    };
    assert_eq!(s.build_display_text("wxyz"), "wxyz");
}

#[test]
fn update_full_session_text_adopts_first_preview() {
    let mut s = State::default();
    s.update_full_session_text("Hello");
    assert_eq!(s.full_session_text, "Hello");
}

#[test]
fn update_full_session_text_grows_on_perfect_extension() {
    let mut s = State {
        full_session_text: "Hello".to_string(),
        ..Default::default()
    };
    s.update_full_session_text("Hello world");
    assert_eq!(s.full_session_text, "Hello world");
}

#[test]
fn update_full_session_text_extends_via_tail_match() {
    let mut s = State {
        full_session_text: "Hello engi".to_string(),
        ..Default::default()
    };
    s.update_full_session_text("engineer here");
    assert_eq!(s.full_session_text, "Hello engineer here");
}

#[test]
fn update_with_stabilization_locks_common_prefix_across_two_texts() {
    let mut s = State::default();
    s.update_with_stabilization("Hello world");
    s.update_with_stabilization("Hello there");
    // The two texts share the char-prefix "Hello ", which stabilizes.
    assert_eq!(s.stabilized_text, "Hello ");
    // Session text was seeded by the first (longer) preview and not shrunk.
    assert_eq!(s.full_session_text, "Hello world");
}

// ---------------------------------------------------------------------------
// type_notice (fixed failure markers)
// ---------------------------------------------------------------------------

/// The markers are constants we control, so the sanitizer is a no-op on them
/// today. Assert it anyway: this is what keeps the property true by
/// construction if someone edits the strings later.
#[test]
fn notice_constants_survive_the_sanitizer_unchanged() {
    for n in notice::ALL {
        let sanitized: String = n
            .chars()
            .filter(|&c| !crate::output::preview::is_unsafe_to_type(c))
            .collect();
        assert_eq!(
            &sanitized, n,
            "notice contains a character that must never be typed: {n:?}"
        );
    }
}

/// A notice is typed verbatim — no capitalization, no trailing period, no
/// trailing space. `process_final_text` applies all three, which is exactly why
/// a notice must not go through it.
// `start_paused` so the notice's key-release delay is virtual — this test
// asserts what gets typed, not how long it waits.
#[tokio::test(start_paused = true)]
async fn type_notice_types_the_marker_verbatim() {
    let (sim, buf) = Simulator::capture();
    let mut typer = Typer::new(sim);

    typer.type_notice(notice::NO_MODEL_LOADED).await;

    assert_eq!(*buf.lock().unwrap(), "[Super STT: no model loaded]");
}

/// Transcript state feeds preview tail-matching on the *next* recording. If a
/// notice landed in it, the typer would try to extend "[Super STT: …]" into the
/// following sentence.
// `start_paused` so the notice's key-release delay is virtual — this test
// asserts what gets typed, not how long it waits.
#[tokio::test(start_paused = true)]
async fn type_notice_leaves_transcript_state_untouched() {
    let (sim, _buf) = Simulator::capture();
    let mut typer = Typer::new(sim);

    typer.type_notice(notice::TRANSCRIPTION_FAILED).await;

    assert_eq!(typer.state.last_transcription, "");
    assert_eq!(typer.state.prev_text, "");
    assert_eq!(typer.state.full_session_text, "");
}

/// A notice can be typed within milliseconds of the hotkey press (the no-model
/// preflight rejects before capture starts), so the shortcut's modifiers are
/// often still held. Typing then would deliver modified keystrokes and fire
/// shortcuts in the user's application instead of inserting text. The wait must
/// therefore happen BEFORE the text reaches the simulator, not after.
///
/// `start_paused` auto-advances tokio's clock while the runtime is idle, so
/// this asserts the full delay elapsed without spending it in wall-clock time.
#[tokio::test(start_paused = true)]
async fn type_notice_waits_for_shortcut_keys_to_be_released_before_typing() {
    let (sim, buf) = Simulator::capture();
    let mut typer = Typer::new(sim);
    let start = tokio::time::Instant::now();

    typer.type_notice(notice::NO_MODEL_LOADED).await;

    assert!(
        start.elapsed() >= std::time::Duration::from_secs(1),
        "notice was typed after only {:?} — modifiers may still be held",
        start.elapsed()
    );
    assert_eq!(*buf.lock().unwrap(), "[Super STT: no model loaded]");
}
