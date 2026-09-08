// SPDX-License-Identifier: GPL-3.0-only
use super::*;

#[test]
fn test_preprocess_text() {
    // Basic functionality
    assert_eq!(preprocess_text("hello world", true), "Hello world");
    assert_eq!(preprocess_text("hello world", false), "Hello world.");
    assert_eq!(preprocess_text("", true), "");

    assert_eq!(preprocess_text("...hello world", true), "Hello world");
    assert_eq!(preprocess_text("  ...  hello world  ", true), "Hello world");
    assert_eq!(
        preprocess_text("  multiple   spaces  ", true),
        "Multiple spaces"
    );
}

#[test]
fn preprocess_text_strips_unsafe_control_and_format_chars() {
    // Non-whitespace C0/C1 controls (ESC, BEL, NUL, backspace) are removed.
    assert_eq!(preprocess_text("he\u{1b}[31mllo\u{07}", true), "He[31mllo");
    assert_eq!(preprocess_text("a\u{00}b\u{08}c", true), "Abc");
    // Bidi overrides and zero-width chars are removed.
    assert_eq!(preprocess_text("ab\u{202e}cd", true), "Abcd");
    assert_eq!(preprocess_text("wor\u{200b}d", true), "Word");
    // Real whitespace (newline/tab) is preserved as a word separator, not glued.
    assert_eq!(
        preprocess_text("hello\nworld\tfoo", true),
        "Hello world foo"
    );
}

#[test]
fn test_find_common_prefix() {
    assert_eq!(find_common_prefix("hello world", "hello there"), 6);
    assert_eq!(find_common_prefix("abc", "def"), 0);
    assert_eq!(find_common_prefix("same text", "same text"), 9);
}

#[test]
fn normalize_text_keeps_the_case_it_was_given() {
    assert_eq!(normalize_text("  ...  hello   World  "), "hello World");
    assert_eq!(normalize_text("he\u{1b}llo"), "hello");
    assert_eq!(normalize_text("   "), "");
}

#[test]
fn capitalize_first_uppercases_only_the_first_char() {
    assert_eq!(capitalize_first(""), "");
    assert_eq!(capitalize_first("hello World"), "Hello World");
    // ASCII only, as the transcript always was.
    assert_eq!(capitalize_first("élan"), "élan");
}

// ---------------------------------------------------------------------------
// Window stitching
// ---------------------------------------------------------------------------

/// The shared text is the end of the transcript and the start of the window.
#[test]
fn find_overlap_locates_the_shared_text_at_the_seam() {
    let session = "hello my name is jorge";
    let preview = "name is jorge and i like";
    let o = find_overlap(session, preview).expect("the windows share text");
    assert_eq!(&session[..o.session_end], "hello my name is jorge");
    assert_eq!(&preview[o.preview_end..], " and i like");
}

/// A window's first word is capitalized by the model as if it opened a
/// sentence; that must not hide the overlap.
#[test]
fn find_overlap_ignores_case() {
    let session = "hello my name is Jorge";
    let preview = "Name is Jorge and I like";
    let o = find_overlap(session, preview).expect("case must not matter");
    assert_eq!(&preview[o.preview_end..], " and I like");
}

/// A run that ends well before the transcript's end is a coincidence, not the
/// seam: stitching on it would cut the transcript there.
#[test]
fn find_overlap_rejects_a_run_far_from_the_seam() {
    assert_eq!(
        find_overlap(
            "the cat sat on the mat and then it slept for a very long time in the warm afternoon sun",
            "the cat sat purring"
        ),
        None
    );
}

/// A small model rewords both ends of the shared stretch. "I'm trying
/// whisper. It's more breathing timing" and "And a whisper, a small breathing
/// tiny model" share "breathing" thirty characters short of the transcript's
/// end and thirty into the window, and that is still the seam.
#[test]
fn find_overlap_survives_a_reworded_seam() {
    let session = "I'm trying whisper. It's more breathing timing";
    let preview = "And a whisper, a small breathing tiny model";
    let o = find_overlap(session, preview).expect("breathing is shared");
    // The run is "breathing" plus whatever its neighbours happen to share
    // ("timing" and "tiny" both start with "ti"); where exactly it ends is
    // immaterial as long as the transcript keeps its text up to there and the
    // window supplies the rest.
    assert!(
        session[..o.session_end].contains("more breathing"),
        "run ends at {:?}",
        &session[..o.session_end]
    );
    assert!(
        preview[o.preview_end..].ends_with("ny model"),
        "window continues with {:?}",
        &preview[o.preview_end..]
    );
}

/// A whole word exactly at the seam is the seam, however short.
#[test]
fn find_overlap_accepts_a_whole_word_exactly_at_the_seam() {
    let o = find_overlap("i went to the", "the store").expect("the seam word");
    assert_eq!(o.session_end, 13);
    assert_eq!(o.preview_end, 3);
    // The transcript's closing period does not move the seam.
    let o = find_overlap("saying to text.", "Text right now").expect("the seam word");
    assert_eq!(o.session_end, 14);
    assert_eq!(o.preview_end, 4);
}

/// Short runs that do not sit at the seam are still coincidences.
#[test]
fn find_overlap_rejects_a_short_run_away_from_the_seam() {
    // "like" ends the transcript but the window does not start with it.
    assert_eq!(find_overlap("and i like", "we like it"), None);
    // Two characters are never enough, even at the seam.
    assert_eq!(find_overlap("i went to", "to be"), None);
}

/// Two equally long runs: "hello " at the window's start and " world" deeper
/// in. The window's head is the shared audio, so the first wins — picking the
/// second would splice nothing on and freeze the preview.
#[test]
fn find_overlap_prefers_the_run_at_the_windows_start() {
    let o = find_overlap("hello world", "hello there world").expect("runs exist");
    assert_eq!(o.session_end, 6);
    assert_eq!(o.preview_end, 6);
}

/// The offsets are byte offsets: slicing with them must not split a multibyte
/// char.
#[test]
fn find_overlap_returns_byte_offsets_safe_for_slicing() {
    let session = "café au lait";
    let preview = "au lait please";
    let o = find_overlap(session, preview).expect("shared text");
    assert_eq!(&session[..o.session_end], "café au lait");
    assert_eq!(&preview[o.preview_end..], " please");
}

#[test]
fn merge_starts_the_transcript_from_the_first_window() {
    assert_eq!(merge_window_preview("", "hello"), "hello");
}

/// Early in a take every window is a re-read of the whole take so far.
#[test]
fn merge_extends_when_the_window_is_a_re_read_of_the_whole_take() {
    assert_eq!(
        merge_window_preview("hello my name", "hello my name is jorge"),
        "hello my name is jorge"
    );
}

/// The model gave the first window lowercase and the second a capital. Same
/// speech; the second extends the first. This used to type "After After".
#[test]
fn merge_extends_when_the_re_read_differs_only_in_case() {
    assert_eq!(
        merge_window_preview("after", "After doing the first review"),
        "After doing the first review"
    );
}

/// A window that ends at a pause gets a period the next re-read does not
/// repeat mid-sentence.
#[test]
fn merge_extends_past_the_windows_closing_punctuation() {
    assert_eq!(
        merge_window_preview("after doing.", "After doing the first review"),
        "After doing the first review"
    );
}

#[test]
fn merge_keeps_the_transcript_when_a_shorter_re_read_differs_only_in_case() {
    assert_eq!(
        merge_window_preview("After doing the first review", "after."),
        "After doing the first review"
    );
}

#[test]
fn merge_keeps_the_transcript_when_the_window_is_a_shorter_re_read() {
    assert_eq!(
        merge_window_preview("hello my name is jorge", "hello my name"),
        "hello my name is jorge"
    );
}

#[test]
fn merge_continues_from_the_shared_text() {
    assert_eq!(
        merge_window_preview("hello my name is jorge", "name is jorge and i like"),
        "hello my name is jorge and i like"
    );
}

/// The previous window ended mid-word. The new window heard the whole word,
/// and its reading replaces the fragment.
#[test]
fn merge_takes_the_windows_reading_of_a_word_cut_at_the_seam() {
    assert_eq!(
        merge_window_preview("hello my name is jor", "my name is jorge and i"),
        "hello my name is jorge and i"
    );
}

/// A window that ends inside the shared text has nothing new; the transcript
/// must not lose what it already had.
#[test]
fn merge_never_shrinks_the_transcript() {
    assert_eq!(
        merge_window_preview("hello my name is jorge", "name is jorge"),
        "hello my name is jorge"
    );
    assert_eq!(
        merge_window_preview("my name is jorge washington", "name is jorge"),
        "my name is jorge washington"
    );
}

/// What the whisper take produced as "…to text. text right now".
#[test]
fn merge_stitches_a_whole_word_at_the_seam() {
    assert_eq!(
        merge_window_preview("what I'm saying to text.", "Text right now"),
        "what I'm saying to text right now"
    );
}

#[test]
fn merge_continues_from_a_reworded_seam() {
    assert_eq!(
        merge_window_preview(
            "I'm trying whisper. It's more breathing timing",
            "And a whisper, a small breathing tiny model"
        ),
        "I'm trying whisper. It's more breathing tiny model"
    );
}

/// No shared text at all: append rather than stall. The stitched preview may
/// repeat a word; a preview that stops growing looks like recognition died.
#[test]
fn merge_appends_when_nothing_is_shared() {
    assert_eq!(
        merge_window_preview("hello there", "how are you"),
        "hello there how are you"
    );
}
