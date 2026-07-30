// SPDX-License-Identifier: GPL-3.0-only

use super::wire_enum::wire_enum_strings;
use strum::IntoEnumIterator;
use strum_macros::{AsRefStr, EnumCount, EnumIter, VariantArray, VariantNames};

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    AsRefStr,
    EnumCount,
    EnumIter,
    VariantArray,
    VariantNames,
    Default,
)]
pub enum AudioTheme {
    #[default]
    Classic, // Original ascending/descending beeps
    Gentle,  // Soft, pleasant tones
    Minimal, // Single short beeps
    SciFi,   // Futuristic computer sounds
    Musical, // Musical chord progressions
    Nature,  // Natural, organic sounds
    Retro,   // 8-bit style sounds
    Silent,  // No audio feedback
}

// `SciFi` intentionally maps to the single-word `scifi` (not `sci_fi`); the
// rest are already single-word snake tokens.
wire_enum_strings!(AudioTheme {
    Classic => "classic",
    Gentle => "gentle",
    Minimal => "minimal",
    SciFi => "scifi",
    Musical => "musical",
    Nature => "nature",
    Retro => "retro",
    Silent => "silent",
});

impl AudioTheme {
    // Additional helpers below

    #[must_use]
    pub fn pretty_name(&self) -> String {
        match self {
            AudioTheme::Classic => "Classic".to_string(),
            AudioTheme::Gentle => "Gentle".to_string(),
            AudioTheme::Minimal => "Minimal".to_string(),
            AudioTheme::SciFi => "Sci-Fi".to_string(),
            AudioTheme::Musical => "Musical".to_string(),
            AudioTheme::Nature => "Nature".to_string(),
            AudioTheme::Retro => "Retro".to_string(),
            AudioTheme::Silent => "Silent".to_string(),
        }
    }

    /// Get all available audio themes
    #[must_use]
    pub fn all_themes() -> Vec<AudioTheme> {
        AudioTheme::iter().collect()
    }

    /// Get start sound frequencies, timings, and fade settings for this theme
    #[must_use]
    pub fn start_sound(&self) -> (Vec<f32>, u64, u64, u64) {
        // Returns (frequencies, duration_ms, fade_in_ms, fade_out_ms)
        match self {
            AudioTheme::Classic => (vec![440.0, 554.0, 659.0], 150, 5, 15),
            AudioTheme::Gentle => (vec![261.6, 329.6, 392.0], 200, 10, 15),
            AudioTheme::Minimal => (vec![262.0], 100, 5, 25),
            AudioTheme::SciFi => (vec![400.0, 600.0, 900.0, 1100.0], 160, 15, 40),
            AudioTheme::Musical => (vec![261.6, 329.6, 392.0, 523.3], 180, 5, 15),
            AudioTheme::Nature => (vec![174.6, 220.0, 261.6], 250, 10, 15),
            AudioTheme::Retro => (vec![659.3, 1118.5, 1575.5], 120, 15, 15),
            AudioTheme::Silent => (vec![], 0, 0, 0), // No sound, no duration
        }
    }

    /// Get end sound frequencies, timings, and fade settings for this theme
    #[must_use]
    pub fn end_sound(&self) -> (Vec<f32>, u64, u64, u64) {
        // Returns (frequencies, duration_ms, fade_in_ms, fade_out_ms)
        match self {
            AudioTheme::Classic => (vec![659.0, 554.0, 441.0], 150, 5, 15),
            AudioTheme::Gentle => (vec![392.0, 329.6, 261.6], 200, 10, 15),
            AudioTheme::Minimal => (vec![523.0], 100, 5, 25),
            AudioTheme::SciFi => (vec![1100.0, 900.0, 600.0, 400.0], 160, 40, 15),
            AudioTheme::Musical => (vec![523.3, 392.0, 329.6, 261.6], 180, 5, 15),
            AudioTheme::Nature => (vec![261.6, 220.0, 174.6], 250, 10, 15),
            AudioTheme::Retro => (vec![1575.5, 1118.5, 659.3], 120, 15, 15),
            AudioTheme::Silent => (vec![], 0, 0, 0), // No sound, no duration
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_theme_display() {
        assert_eq!(AudioTheme::Classic.to_string(), "classic");
        assert_eq!(AudioTheme::Gentle.to_string(), "gentle");
        assert_eq!(AudioTheme::Minimal.to_string(), "minimal");
        assert_eq!(AudioTheme::SciFi.to_string(), "scifi");
        assert_eq!(AudioTheme::Musical.to_string(), "musical");
        assert_eq!(AudioTheme::Nature.to_string(), "nature");
        assert_eq!(AudioTheme::Retro.to_string(), "retro");
        assert_eq!(AudioTheme::Silent.to_string(), "silent");
    }

    #[test]
    fn test_audio_theme_from_str() {
        assert_eq!(
            "classic".parse::<AudioTheme>().unwrap(),
            AudioTheme::Classic
        );
        assert_eq!("scifi".parse::<AudioTheme>().unwrap(), AudioTheme::SciFi);
        // Exact snake token only: no case-folding, no fallback-to-default. An
        // unknown value errors so REST callers can 400 (the config layer's
        // `deserialize_or_default` handles load resilience separately).
        assert!("GENTLE".parse::<AudioTheme>().is_err());
        assert!("unknown".parse::<AudioTheme>().is_err());
    }

    #[test]
    fn test_audio_theme_pretty_names() {
        assert_eq!(AudioTheme::Classic.pretty_name(), "Classic");
        assert_eq!(AudioTheme::SciFi.pretty_name(), "Sci-Fi");
        assert_eq!(AudioTheme::Silent.pretty_name(), "Silent");
    }

    #[test]
    fn test_all_themes_count() {
        let themes = AudioTheme::all_themes();
        assert_eq!(themes.len(), 8);
        assert!(themes.contains(&AudioTheme::Classic));
        assert!(themes.contains(&AudioTheme::Silent));
    }

    #[test]
    fn test_silent_theme_properties() {
        let (frequencies, duration, fade_in, fade_out) = AudioTheme::Silent.start_sound();
        assert!(frequencies.is_empty());
        assert_eq!(duration, 0);
        assert_eq!(fade_in, 0);
        assert_eq!(fade_out, 0);

        let (end_frequencies, end_duration, end_fade_in, end_fade_out) =
            AudioTheme::Silent.end_sound();
        assert!(end_frequencies.is_empty());
        assert_eq!(end_duration, 0);
        assert_eq!(end_fade_in, 0);
        assert_eq!(end_fade_out, 0);
    }

    #[test]
    fn test_non_silent_themes_have_audio() {
        for theme in AudioTheme::all_themes() {
            if theme != AudioTheme::Silent {
                let (frequencies, duration, fade_in, fade_out) = theme.start_sound();
                assert!(
                    !frequencies.is_empty(),
                    "Theme {theme:?} should have start frequencies"
                );
                assert!(
                    duration > 0,
                    "Theme {theme:?} should have positive duration"
                );
                assert!(
                    fade_in < duration,
                    "Theme {theme:?} fade_in should be less than duration"
                );
                assert!(
                    fade_out < duration,
                    "Theme {theme:?} fade_out should be less than duration"
                );

                let (end_frequencies, end_duration, end_fade_in, end_fade_out) = theme.end_sound();
                assert!(
                    !end_frequencies.is_empty(),
                    "Theme {theme:?} should have end frequencies"
                );
                assert!(
                    end_duration > 0,
                    "Theme {theme:?} should have positive end duration"
                );
                assert!(
                    end_fade_in < end_duration,
                    "Theme {theme:?} end fade_in should be less than duration"
                );
                assert!(
                    end_fade_out < end_duration,
                    "Theme {theme:?} end fade_out should be less than duration"
                );
            }
        }
    }

    #[test]
    fn test_frequency_ranges() {
        for theme in AudioTheme::all_themes() {
            if theme != AudioTheme::Silent {
                let (frequencies, _, _, _) = theme.start_sound();
                for &freq in &frequencies {
                    assert!(
                        (100.0..=5000.0).contains(&freq),
                        "Theme {theme:?} frequency {freq}Hz should be in audible range (100-5000Hz)"
                    );
                }

                let (end_frequencies, _, _, _) = theme.end_sound();
                for &freq in &end_frequencies {
                    assert!(
                        (100.0..=5000.0).contains(&freq),
                        "Theme {theme:?} end frequency {freq}Hz should be in audible range (100-5000Hz)"
                    );
                }
            }
        }
    }

    #[test]
    fn test_classic_theme_ascending_descending() {
        let (start_freqs, _, _, _) = AudioTheme::Classic.start_sound();
        let (end_freqs, _, _, _) = AudioTheme::Classic.end_sound();

        // Classic should have ascending start (440 -> 554 -> 659)
        assert!(start_freqs[0] < start_freqs[1]);
        assert!(start_freqs[1] < start_freqs[2]);

        // Classic should have descending end (659 -> 554 -> 441)
        assert!(end_freqs[0] > end_freqs[1]);
        assert!(end_freqs[1] > end_freqs[2]);
    }

    #[test]
    fn test_retro_theme_frequency_progression() {
        let (frequencies, _, _, _) = AudioTheme::Retro.start_sound();

        // Retro theme should have ascending frequencies (8-bit style)
        assert!(
            frequencies.len() >= 2,
            "Retro theme should have multiple frequencies"
        );

        // Check that frequencies generally increase (8-bit ascending pattern)
        for i in 1..frequencies.len() {
            assert!(
                frequencies[i] > frequencies[i - 1],
                "Retro theme frequencies should ascend: {}Hz -> {}Hz",
                frequencies[i - 1],
                frequencies[i]
            );
        }

        // Last frequency should be quite high (8-bit style)
        let last_freq = frequencies[frequencies.len() - 1];
        assert!(
            last_freq > 1000.0,
            "Retro theme should end with high frequency, got {last_freq}Hz"
        );
    }

    #[test]
    fn test_serde_serialization() {
        let theme = AudioTheme::SciFi;
        let serialized = serde_json::to_string(&theme).unwrap();
        let deserialized: AudioTheme = serde_json::from_str(&serialized).unwrap();
        assert_eq!(theme, deserialized);
    }

    #[test]
    fn test_default_theme() {
        assert_eq!(AudioTheme::default(), AudioTheme::Classic);
    }
}
