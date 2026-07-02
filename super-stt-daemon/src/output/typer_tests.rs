// SPDX-License-Identifier: GPL-3.0-only
use super::*;

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
    let mut s = State::default();
    s.stabilized_text = "Hello engi".to_string();
    // "engi"'s tail "ngi" overlaps "engineer…"; suffix "neer is good" is grafted on.
    assert_eq!(
        s.build_display_text("engineer is good"),
        "Hello engineer is good"
    );
}

#[test]
fn build_display_text_prefers_session_when_no_tail_match_and_session_longer() {
    let mut s = State::default();
    s.stabilized_text = "abc".to_string();
    s.full_session_text = "abcdefghij".to_string();
    assert_eq!(s.build_display_text("wxyz"), "abcdefghij");
}

#[test]
fn build_display_text_prefers_preview_when_no_tail_match_and_preview_longer() {
    let mut s = State::default();
    s.stabilized_text = "abc".to_string();
    s.full_session_text = "ab".to_string();
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
    let mut s = State::default();
    s.full_session_text = "Hello".to_string();
    s.update_full_session_text("Hello world");
    assert_eq!(s.full_session_text, "Hello world");
}

#[test]
fn update_full_session_text_extends_via_tail_match() {
    let mut s = State::default();
    s.full_session_text = "Hello engi".to_string();
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
