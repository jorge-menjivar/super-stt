// SPDX-License-Identifier: GPL-3.0-only
//! Where the daemon reads the text it speaks for `POST /v1/tts/speak/read`.

use super_engine_protocol::wire_enum_strings;

/// Where the daemon reads the text that the read-aloud shortcut speaks.
///
/// Text to speech's counterpart to
/// [`WriteMethod`](crate::models::write_method::WriteMethod): that one decides
/// how a transcription reaches the screen, and this one how text on the screen
/// reaches the voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReadMethod {
    /// The highlighted text, Wayland's primary selection: whatever was last
    /// highlighted, in any app, with nothing copied.
    #[default]
    Selection,
    /// The clipboard: whatever was last copied.
    Clipboard,
}

wire_enum_strings!(ReadMethod {
    Selection => "selection",
    Clipboard => "clipboard",
});

impl ReadMethod {
    /// Every method, in the order a picker lists them.
    pub const ALL: [Self; 2] = [Self::Selection, Self::Clipboard];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_selection() {
        assert_eq!(ReadMethod::default(), ReadMethod::Selection);
    }

    #[test]
    fn wire_tokens_round_trip() {
        for method in ReadMethod::ALL {
            assert_eq!(method.to_string().parse::<ReadMethod>(), Ok(method));
        }
        assert_eq!(ReadMethod::Selection.to_string(), "selection");
        assert_eq!(ReadMethod::Clipboard.to_string(), "clipboard");
    }
}
