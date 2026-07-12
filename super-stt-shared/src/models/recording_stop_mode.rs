// SPDX-License-Identifier: GPL-3.0-only

use super::wire_enum::wire_enum_strings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecordingStopMode {
    /// Recording stops only when silence is detected.
    SilenceOnly,
    /// Recording stops on silence detection or manual shortcut press.
    #[default]
    SilenceAndManual,
    /// Recording stops only via manual shortcut press.
    ManualOnly,
}

wire_enum_strings!(RecordingStopMode {
    SilenceOnly => "silence_only",
    SilenceAndManual => "silence_and_manual",
    ManualOnly => "manual_only",
});

impl RecordingStopMode {
    /// Whether silence detection should be active in this mode.
    #[must_use]
    pub fn silence_detection_enabled(self) -> bool {
        matches!(self, Self::SilenceOnly | Self::SilenceAndManual)
    }

    /// Whether pressing the shortcut a second time should stop recording.
    #[must_use]
    pub fn manual_stop_enabled(self) -> bool {
        matches!(self, Self::SilenceAndManual | Self::ManualOnly)
    }

    /// Human-readable label for UI display.
    #[must_use]
    pub fn pretty_name(self) -> &'static str {
        match self {
            Self::SilenceOnly => "Silence Detection Only",
            Self::SilenceAndManual => "Silence Detection + Manual Stop",
            Self::ManualOnly => "Manual Stop Only",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_silence_and_manual() {
        assert_eq!(
            RecordingStopMode::default(),
            RecordingStopMode::SilenceAndManual
        );
    }

    #[test]
    fn display_roundtrip() {
        for mode in [
            RecordingStopMode::SilenceOnly,
            RecordingStopMode::SilenceAndManual,
            RecordingStopMode::ManualOnly,
        ] {
            let s = mode.to_string();
            let parsed: RecordingStopMode = s.parse().unwrap();
            assert_eq!(parsed, mode);
        }
    }

    #[test]
    fn wire_tokens_are_snake_case() {
        assert_eq!(RecordingStopMode::SilenceOnly.to_string(), "silence_only");
        assert_eq!(
            RecordingStopMode::SilenceAndManual.to_string(),
            "silence_and_manual"
        );
        assert_eq!(RecordingStopMode::ManualOnly.to_string(), "manual_only");
    }

    #[test]
    fn from_str_rejects_unknown_and_dropped_aliases() {
        assert!("nonsense".parse::<RecordingStopMode>().is_err());
        // Former aliases are gone (no legacy aliases).
        for dropped in ["silence", "both", "manual", "silence-only", "manual-only"] {
            assert!(
                dropped.parse::<RecordingStopMode>().is_err(),
                "`{dropped}` must no longer parse"
            );
        }
    }

    #[test]
    fn silence_detection_flags() {
        assert!(RecordingStopMode::SilenceOnly.silence_detection_enabled());
        assert!(RecordingStopMode::SilenceAndManual.silence_detection_enabled());
        assert!(!RecordingStopMode::ManualOnly.silence_detection_enabled());
    }

    #[test]
    fn manual_stop_flags() {
        assert!(!RecordingStopMode::SilenceOnly.manual_stop_enabled());
        assert!(RecordingStopMode::SilenceAndManual.manual_stop_enabled());
        assert!(RecordingStopMode::ManualOnly.manual_stop_enabled());
    }

    #[test]
    fn serde_roundtrip() {
        let mode = RecordingStopMode::ManualOnly;
        let json = serde_json::to_string(&mode).unwrap();
        let parsed: RecordingStopMode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, mode);
    }
}
