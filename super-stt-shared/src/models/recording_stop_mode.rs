// SPDX-License-Identifier: GPL-3.0-only

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RecordingStopMode {
    /// Recording stops only when silence is detected.
    SilenceOnly,
    /// Recording stops on silence detection or manual shortcut press.
    #[default]
    SilenceAndManual,
    /// Recording stops only via manual shortcut press.
    ManualOnly,
}

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

impl std::fmt::Display for RecordingStopMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SilenceOnly => write!(f, "silence-only"),
            Self::SilenceAndManual => write!(f, "silence-and-manual"),
            Self::ManualOnly => write!(f, "manual-only"),
        }
    }
}

impl std::str::FromStr for RecordingStopMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "silence-only" | "silence" => Ok(Self::SilenceOnly),
            "silence-and-manual" | "both" => Ok(Self::SilenceAndManual),
            "manual-only" | "manual" => Ok(Self::ManualOnly),
            other => Err(format!("unknown recording stop mode: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_silence_and_manual() {
        assert_eq!(RecordingStopMode::default(), RecordingStopMode::SilenceAndManual);
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
    fn from_str_short_aliases() {
        assert_eq!("silence".parse::<RecordingStopMode>().unwrap(), RecordingStopMode::SilenceOnly);
        assert_eq!("both".parse::<RecordingStopMode>().unwrap(), RecordingStopMode::SilenceAndManual);
        assert_eq!("manual".parse::<RecordingStopMode>().unwrap(), RecordingStopMode::ManualOnly);
    }

    #[test]
    fn from_str_invalid() {
        assert!("nonsense".parse::<RecordingStopMode>().is_err());
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
