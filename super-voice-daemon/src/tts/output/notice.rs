// SPDX-License-Identifier: GPL-3.0-only
//! What a synthesis failure looks like to the user.
//!
//! A failure reaches the user as a desktop notification, which has a summary
//! and a body of its own and carries its app name and icon separately — so the
//! summary names the failure and the body gives the reason, including the
//! reasons a backend authored, which is where most of them come from. Backends
//! are explicitly untrusted (audit 2 Tier 3 #8) and a notification server may
//! render limited markup in the body, so that text is flattened, escaped, and
//! clamped by [`sanitize`] before it is handed over, and labelled so a relayed
//! failure is never read as one of the daemon's own.
//!
//! The STT build had a second channel — typing the notice into the focused
//! window — because its output was already going there. This daemon's output is
//! audio, so a failure has nowhere to be typed and one channel is all there is.

/// Who authored a failure's detail, which decides how the notification body
/// labels it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    /// The daemon itself: its own preconditions, the audio device, the pipeline.
    Daemon,
    /// The backend serving the model.
    Backend,
}

/// Longest reason a notification body carries. A bubble is not a log view, and
/// a backend chooses its own message length.
const MAX_DETAIL: usize = 300;

/// One synthesis failure, rendered for the notification channel.
pub(crate) struct Failure {
    /// Notification summary: what failed. It deliberately omits the app name —
    /// the notification already carries `Super TTS` as its app name and icon,
    /// and repeating it costs the user the only line they are certain to read.
    pub(crate) summary: &'static str,
    /// Notification body: why it failed.
    pub(crate) body: String,
}

impl Failure {
    /// No model is loaded. There is no reason to report beyond the fact itself,
    /// so the body says what to do about it.
    pub(crate) fn no_model_loaded() -> Self {
        Self {
            summary: "No model loaded",
            body: "Load a model and try again.".to_string(),
        }
    }

    /// The output device could not be opened, so there is nowhere to play.
    pub(crate) fn could_not_open_output(detail: &str) -> Self {
        Self {
            summary: "Could not play audio",
            body: body(
                Origin::Daemon,
                detail,
                "The audio output device could not be opened.",
            ),
        }
    }

    /// The backend was reached but produced no usable audio. `origin` is a
    /// parameter here and fixed in every other constructor because this is the
    /// one failure the daemon usually did not cause: the backend answered, and
    /// said no.
    pub(crate) fn synthesis_failed(origin: Origin, detail: &str) -> Self {
        Self {
            summary: "Synthesis failed",
            body: body(origin, detail, "The text could not be synthesized."),
        }
    }
}

/// Render a reason into a notification body: labelled by origin, or replaced by
/// `fallback` when there is nothing left to say after sanitizing.
fn body(origin: Origin, detail: &str, fallback: &str) -> String {
    let detail = sanitize(detail);
    if detail.is_empty() {
        return fallback.to_string();
    }
    match origin {
        Origin::Backend => format!("Backend error: {detail}"),
        Origin::Daemon => detail,
    }
}

/// Reduce a reason to one line of inert plain text.
///
/// Three things happen here, in order:
///
/// - Control characters — including the newlines an error chain is full of —
///   become spaces, and runs of whitespace collapse. A body cannot fake extra
///   lines, and cannot smuggle terminal escapes into a server that logs it.
/// - The result is clamped to [`MAX_DETAIL`] characters, marked with an ellipsis
///   so the user can tell it was cut.
/// - `&`, `<`, and `>` are escaped last, so a clamp can never sever an entity.
///   Servers that advertise `body-markup` render the body as markup, and some
///   render anchors: unescaped, a backend could put a clickable link of its
///   choosing inside a notification wearing this daemon's name and icon. The
///   cost is that a literal `&` in a URL shows as `&amp;` on the servers that
///   do not render markup, which is the cheaper of the two failures.
fn sanitize(detail: &str) -> String {
    let flattened: String = detail
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut one_line = flattened.split_whitespace().collect::<Vec<_>>().join(" ");

    if one_line.chars().count() > MAX_DETAIL {
        one_line = one_line.chars().take(MAX_DETAIL - 1).collect::<String>();
        one_line.push('…');
    }

    one_line
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::{Failure, MAX_DETAIL, Origin, sanitize};

    /// Every failure the daemon can raise, for the invariants that must hold
    /// across all of them.
    fn every_failure() -> Vec<Failure> {
        vec![
            Failure::no_model_loaded(),
            Failure::could_not_open_output("d"),
            Failure::synthesis_failed(Origin::Daemon, "d"),
            Failure::synthesis_failed(Origin::Backend, "d"),
        ]
    }

    /// The reason the user reported: a backend failure the log had in full
    /// showed up as a fixed marker. The body must carry the reason, and must say
    /// whose it is.
    #[test]
    fn a_backend_reason_reaches_the_body_labelled() {
        let f = Failure::synthesis_failed(
            Origin::Backend,
            "Could not reach http://192.168.0.172/v1/audio/speech (write_failed).",
        );
        assert_eq!(f.summary, "Synthesis failed");
        assert_eq!(
            f.body,
            "Backend error: Could not reach http://192.168.0.172/v1/audio/speech (write_failed)."
        );
    }

    /// The daemon's own reasons are not relayed from anywhere, so labelling them
    /// would be noise.
    #[test]
    fn a_daemon_reason_is_not_labelled() {
        let f = Failure::could_not_open_output("Audio device disappeared mid-utterance");
        assert_eq!(f.body, "Audio device disappeared mid-utterance");
    }

    /// A failure that arrives with nothing to report still says something.
    #[test]
    fn an_empty_reason_falls_back_to_a_fixed_sentence() {
        assert_eq!(
            Failure::synthesis_failed(Origin::Backend, "   ").body,
            "The text could not be synthesized."
        );
        assert_eq!(
            Failure::could_not_open_output("").body,
            "The audio output device could not be opened."
        );
    }

    /// No summary repeats the app name: the notification carries it already, and
    /// the summary is the line the user actually reads.
    #[test]
    fn no_summary_repeats_the_app_name() {
        for f in every_failure() {
            assert!(
                !f.summary.contains("Super TTS"),
                "summary repeats the app name: {:?}",
                f.summary
            );
        }
    }

    /// No failure reaches the user with an empty body. Every constructor either
    /// carries a reason or substitutes a sentence, and a bubble whose body is
    /// blank tells the user only that something is broken.
    #[test]
    fn no_failure_has_an_empty_body() {
        for f in every_failure() {
            assert!(!f.body.is_empty(), "empty body for {:?}", f.summary);
        }
        for f in [
            Failure::could_not_open_output(""),
            Failure::synthesis_failed(Origin::Daemon, "\n\t "),
            Failure::synthesis_failed(Origin::Backend, "\u{1b}"),
        ] {
            assert!(
                !f.body.is_empty(),
                "a reason that sanitized away left an empty body: {:?}",
                f.summary
            );
        }
    }

    /// An anyhow chain arrives with newlines in it; a bubble gets one line.
    #[test]
    fn a_multi_line_reason_collapses_to_one_line() {
        assert_eq!(
            sanitize("Failed to synthesize\n\nCaused by:\n    rate mismatch"),
            "Failed to synthesize Caused by: rate mismatch"
        );
    }

    /// Terminal escapes and other C0/C1 controls do not survive.
    #[test]
    fn control_characters_do_not_survive() {
        let s = sanitize("before\u{1b}[31m\u{7}after\u{85}end");
        assert_eq!(s, "before [31m after end");
        assert!(!s.chars().any(char::is_control));
    }

    /// The three markup characters are escaped, so a `body-markup` server
    /// renders a backend's text as text — not as a link of its choosing.
    #[test]
    fn markup_is_escaped() {
        assert_eq!(
            sanitize("<a href=\"http://evil.test\">Click to fix</a> & wait"),
            "&lt;a href=\"http://evil.test\"&gt;Click to fix&lt;/a&gt; &amp; wait"
        );
    }

    /// A long reason is cut, and visibly so. The clamp runs before escaping, so
    /// it can never leave half an entity behind.
    #[test]
    fn a_long_reason_is_clamped_without_severing_an_entity() {
        let s = sanitize(&"&".repeat(MAX_DETAIL * 2));
        assert!(
            s.ends_with('…'),
            "a cut reason must show that it was cut: {s}"
        );
        assert_eq!(
            s.trim_end_matches('…'),
            "&amp;".repeat(MAX_DETAIL - 1),
            "the clamp left a partial entity behind"
        );
    }
}
