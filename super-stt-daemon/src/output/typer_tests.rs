// SPDX-License-Identifier: GPL-3.0-only
use super::*;
use crate::output::keyboard::Simulator;
use crate::output::notice;
use super_stt_shared::models::protocol::PreviewSource;

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

// ---------------------------------------------------------------------------
// Empty-transcript handling
// ---------------------------------------------------------------------------

/// An empty transcript must type nothing at all. The final text is built as
/// `format!("{processed} ")`, so without a guard a silent recording deposits a
/// bare space into whatever the user has focused.
#[tokio::test]
async fn process_final_text_types_nothing_for_an_empty_transcript() {
    let (sim, buf) = Simulator::capture();
    let mut typer = Typer::new(sim);

    typer.process_final_text("").await;

    assert_eq!(*buf.lock().unwrap(), "");
}

/// Whitespace-only is empty for this purpose — the backend returning " " must
/// not type a space either.
#[tokio::test]
async fn process_final_text_types_nothing_for_a_whitespace_transcript() {
    let (sim, buf) = Simulator::capture();
    let mut typer = Typer::new(sim);

    typer.process_final_text("   ").await;

    assert_eq!(*buf.lock().unwrap(), "");
}

/// The guard must not change the normal path.
#[tokio::test]
async fn process_final_text_still_types_a_non_empty_transcript() {
    let (sim, buf) = Simulator::capture();
    let mut typer = Typer::new(sim);

    typer.process_final_text("hello world").await;

    assert!(
        buf.lock().unwrap().contains("Hello world"),
        "expected the transcript to be typed, got {:?}",
        buf.lock().unwrap()
    );
}

/// Skipping the typing must NOT skip the state reset: leftover session text
/// would feed preview tail-matching on the next recording.
#[tokio::test]
async fn process_final_text_resets_state_even_when_it_types_nothing() {
    let (sim, _buf) = Simulator::capture();
    let mut typer = Typer::new(sim);
    typer.state.prev_text = "stale".to_string();
    typer.state.full_session_text = "stale session".to_string();

    typer.process_final_text("").await;

    assert_eq!(typer.state.prev_text, "");
    assert_eq!(typer.state.full_session_text, "");
}

/// The extracted reset is callable on its own — Task 2 uses it on the
/// no-speech path, which does no typing at all.
#[test]
fn reset_after_recording_clears_transcript_state() {
    let (sim, _buf) = Simulator::capture();
    let mut typer = Typer::new(sim);
    typer.state.prev_text = "stale".to_string();
    typer.state.full_session_text = "stale session".to_string();

    typer.reset_after_recording(String::new());

    assert_eq!(typer.state.prev_text, "");
    assert_eq!(typer.state.full_session_text, "");
    assert_eq!(typer.state.last_transcription, "");
}

// ---------------------------------------------------------------------------
// Preview typing keeps its mirror equal to the screen
// ---------------------------------------------------------------------------

/// Type `previews` in order, checking after each that the mirror is exactly
/// what the capture backend holds. Everything downstream — the next diff, the
/// clear at the end of the recording — trusts that equality. Returns the
/// mirror so the caller can go on to clear it.
async fn type_previews(
    typer: &mut Typer,
    screen: &std::sync::Arc<std::sync::Mutex<String>>,
    source: PreviewSource,
    previews: &[&str],
) -> String {
    let mut typed = String::new();
    for preview in previews {
        typer.update_preview(preview, source, &mut typed).await;
        assert_eq!(
            *screen.lock().unwrap(),
            typed,
            "after preview {preview:?} the mirror must be what is on screen"
        );
    }
    typed
}

/// The first text and every extension used to be typed with a trailing space
/// the mirror did not hold, so "hello" then "hello world" put "Hello  world "
/// on screen.
#[tokio::test]
async fn an_extension_appends_only_the_new_words() {
    let (sim, screen) = Simulator::capture();
    let mut typer = Typer::new(sim);

    type_previews(
        &mut typer,
        &screen,
        PreviewSource::Window,
        &["hello", "hello world"],
    )
    .await;

    assert_eq!(*screen.lock().unwrap(), "Hello world");
}

/// A replacement backspaces from the mirror's length. With the mirror short of
/// the screen, too little was deleted and a fragment of the old text stayed:
/// this sequence used to end as "Hello  wthere world".
#[tokio::test]
async fn a_replacement_leaves_no_fragment_of_the_old_text() {
    let (sim, screen) = Simulator::capture();
    let mut typer = Typer::new(sim);

    type_previews(
        &mut typer,
        &screen,
        PreviewSource::Window,
        &["hello", "hello world", "hello there world"],
    )
    .await;

    assert_eq!(*screen.lock().unwrap(), "Hello there world");
}

/// The diff is by character, not byte, so multibyte text is neither split nor
/// over-deleted.
#[tokio::test]
async fn multibyte_text_is_diffed_by_character() {
    let (sim, screen) = Simulator::capture();
    let mut typer = Typer::new(sim);

    type_previews(
        &mut typer,
        &screen,
        PreviewSource::Window,
        &["wörld", "wörld peace", "wörld piece"],
    )
    .await;

    assert_eq!(*screen.lock().unwrap(), "Wörld piece");
}

/// The clear backspaces the mirror's length. When the mirror was short of the
/// screen, the start of the preview survived it — "He" here.
#[tokio::test]
async fn clearing_a_preview_leaves_the_screen_empty() {
    let (sim, screen) = Simulator::capture();
    let mut typer = Typer::new(sim);
    let mut typed = type_previews(
        &mut typer,
        &screen,
        PreviewSource::Window,
        &["hello", "hello world", "hello there world"],
    )
    .await;

    typer.clear_preview(&mut typed).await;

    assert_eq!(*screen.lock().unwrap(), "");
    assert_eq!(typed, "");
}

/// What the user actually saw: the leftover of the preview glued to the front
/// of the final transcript ("HeHello there world. ").
#[tokio::test]
async fn the_final_transcript_follows_a_cleared_preview_with_nothing_in_between() {
    let (sim, screen) = Simulator::capture();
    let mut typer = Typer::new(sim);
    let mut typed = type_previews(
        &mut typer,
        &screen,
        PreviewSource::Window,
        &["hello", "hello there world"],
    )
    .await;

    typer.clear_preview(&mut typed).await;
    typer.process_final_text("hello there world").await;

    assert_eq!(*screen.lock().unwrap(), "Hello there world. ");
}

// ---------------------------------------------------------------------------
// What the screen shows is the transcript so far
// ---------------------------------------------------------------------------

/// A stream frame is the transcript so far. The old three-char graft found the
/// rightmost "the" and dropped "mat and", showing "The cat sat on the dog".
#[tokio::test]
async fn a_stream_preview_is_the_transcript_so_far() {
    let (sim, screen) = Simulator::capture();
    let mut typer = Typer::new(sim);

    type_previews(
        &mut typer,
        &screen,
        PreviewSource::Stream,
        &["the cat sat on the", "the cat sat on the mat and the dog"],
    )
    .await;

    assert_eq!(
        *screen.lock().unwrap(),
        "The cat sat on the mat and the dog"
    );
}

/// A backend may revise what it already said. The screen follows, retyped from
/// the first character that changed.
#[tokio::test]
async fn a_stream_revision_is_retyped_from_the_change() {
    let (sim, screen) = Simulator::capture();
    let mut typer = Typer::new(sim);

    type_previews(
        &mut typer,
        &screen,
        PreviewSource::Stream,
        &[
            "the cat sat on the mat and the dog",
            "the cat sat on the mat, and the dog barked",
        ],
    )
    .await;

    assert_eq!(
        *screen.lock().unwrap(),
        "The cat sat on the mat, and the dog barked"
    );
}

/// Sliding windows: the first two are re-reads of the whole take, the rest are
/// the last few seconds each, overlapping the one before. The screen shows one
/// transcript, not the latest window.
#[tokio::test]
async fn sliding_windows_are_stitched_into_one_transcript() {
    let (sim, screen) = Simulator::capture();
    let mut typer = Typer::new(sim);

    type_previews(
        &mut typer,
        &screen,
        PreviewSource::Window,
        &[
            "hello my name",
            "hello my name is jorge",
            "name is jorge and i like yellow",
            "like yellow cats a lot",
        ],
    )
    .await;

    assert_eq!(
        *screen.lock().unwrap(),
        "Hello my name is jorge and i like yellow cats a lot"
    );
}

/// The model capitalizes a window's first word as if it opened a sentence.
/// The transcript keeps its own reading of the shared text, so that capital
/// never lands mid-sentence on screen.
#[tokio::test]
async fn a_windows_opening_capital_stays_off_the_screen() {
    let (sim, screen) = Simulator::capture();
    let mut typer = Typer::new(sim);

    type_previews(
        &mut typer,
        &screen,
        PreviewSource::Window,
        &["hello my name is Jorge", "Name is Jorge and I like"],
    )
    .await;

    assert_eq!(*screen.lock().unwrap(), "Hello my name is Jorge and I like");
}

/// The same window twice — a quiet tick — must not touch the keyboard.
#[tokio::test]
async fn a_repeated_preview_types_nothing() {
    let (sim, screen) = Simulator::capture();
    let mut typer = Typer::new(sim);
    let mut typed = String::new();
    typer
        .update_preview("hello world", PreviewSource::Window, &mut typed)
        .await;
    // Poison the screen: any keystroke now would show up as a difference.
    screen.lock().unwrap().push('!');

    typer
        .update_preview("hello world", PreviewSource::Window, &mut typed)
        .await;

    assert_eq!(*screen.lock().unwrap(), "Hello world!");
}
