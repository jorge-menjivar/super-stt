// SPDX-License-Identifier: GPL-3.0-only
//! Where a preview transcript came from, and so how a client should read it.

use crate::models::wire_enum::wire_enum_strings;

/// Where the text of a `preview` frame, or a `partial_stt` event, came from.
///
/// The two kinds look alike on the wire and mean different things: one frame
/// extends the last, the other replaces it. A client rendering "the transcript
/// so far" cannot do it from window frames, and one rendering "what was just
/// said" gets the whole take from stream frames — so the daemon says which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewSource {
    /// A realtime model's own running transcript. Each frame carries
    /// everything heard so far and extends the one before it.
    Stream,
    /// A sliding window the daemon re-transcribed. Each frame carries only the
    /// last few seconds of capture and replaces the one before it.
    Window,
}

wire_enum_strings!(PreviewSource {
    Stream => "stream",
    Window => "window",
});
