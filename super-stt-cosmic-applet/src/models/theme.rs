// SPDX-License-Identifier: GPL-3.0-only
use cosmic::iced::Color;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum VisualizationTheme {
    Pulse,
    BottomEqualizer,
    #[default]
    CenteredEqualizer,
    Waveform,
}

impl std::fmt::Display for VisualizationTheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VisualizationTheme::Pulse => write!(f, "pulse"),
            VisualizationTheme::BottomEqualizer => write!(f, "b_equalizer"),
            VisualizationTheme::CenteredEqualizer => write!(f, "c_equalizer"),
            VisualizationTheme::Waveform => write!(f, "waveform"),
        }
    }
}

impl std::str::FromStr for VisualizationTheme {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pulse" => Ok(VisualizationTheme::Pulse),
            "b_equalizer" => Ok(VisualizationTheme::BottomEqualizer),
            "c_equalizer" => Ok(VisualizationTheme::CenteredEqualizer),
            "waveform" => Ok(VisualizationTheme::Waveform),
            _ => Err(()),
        }
    }
}

impl VisualizationTheme {
    pub fn pretty_name(&self) -> String {
        match self {
            VisualizationTheme::Pulse => "Pulse".to_string(),
            VisualizationTheme::BottomEqualizer => "Equalizer".to_string(),
            VisualizationTheme::CenteredEqualizer => "Centered Bars".to_string(),
            VisualizationTheme::Waveform => "Waveform".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WorkingAnimationTheme {
    /// Damped water-droplet ripple radiating from the center.
    #[default]
    Droplet,
    /// Glowing dot tracing a sine path with a fading trail.
    Comet,
    /// Three dots pulsing in sequence — a compact loading indicator. Exempt
    /// from the side split: renders centred in full on every applet.
    Dots,
}

impl std::fmt::Display for WorkingAnimationTheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkingAnimationTheme::Droplet => write!(f, "droplet"),
            WorkingAnimationTheme::Comet => write!(f, "comet"),
            WorkingAnimationTheme::Dots => write!(f, "dots"),
        }
    }
}

impl std::str::FromStr for WorkingAnimationTheme {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "droplet" => Ok(WorkingAnimationTheme::Droplet),
            "comet" => Ok(WorkingAnimationTheme::Comet),
            "dots" => Ok(WorkingAnimationTheme::Dots),
            _ => Err(()),
        }
    }
}

impl WorkingAnimationTheme {
    pub fn pretty_name(self) -> String {
        match self {
            WorkingAnimationTheme::Droplet => "Droplet".to_string(),
            WorkingAnimationTheme::Comet => "Comet".to_string(),
            WorkingAnimationTheme::Dots => "Dots".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum VisualizationSide {
    #[default]
    Full,
    Left,
    Right,
}

impl std::fmt::Display for VisualizationSide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VisualizationSide::Full => write!(f, "full"),
            VisualizationSide::Left => write!(f, "left"),
            VisualizationSide::Right => write!(f, "right"),
        }
    }
}

impl std::str::FromStr for VisualizationSide {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "full" => Ok(VisualizationSide::Full),
            "left" => Ok(VisualizationSide::Left),
            "right" => Ok(VisualizationSide::Right),
            _ => Err(()),
        }
    }
}

impl VisualizationSide {
    #[must_use]
    pub fn pretty_name(&self) -> String {
        match self {
            VisualizationSide::Full => "Full Wave".to_string(),
            VisualizationSide::Left => "Left Side".to_string(),
            VisualizationSide::Right => "Right Side".to_string(),
        }
    }
}

/// Where the panel icon sits when visualizations are hidden. Persisted in the
/// applet config as its snake-case wire id (`start`/`center`/`end`) and mapped
/// to an iced [`Alignment`] at render time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum IconAlignment {
    Start,
    Center,
    #[default]
    End,
}

impl IconAlignment {
    /// Human-facing label for the segmented selector.
    pub fn pretty_name(self) -> &'static str {
        match self {
            IconAlignment::Start => "Start",
            IconAlignment::Center => "Center",
            IconAlignment::End => "End",
        }
    }

    /// Map to the iced cross-axis alignment used when placing the panel icon.
    pub fn to_alignment(self) -> cosmic::iced::Alignment {
        match self {
            IconAlignment::Start => cosmic::iced::Alignment::Start,
            IconAlignment::Center => cosmic::iced::Alignment::Center,
            IconAlignment::End => cosmic::iced::Alignment::End,
        }
    }
}

impl std::fmt::Display for IconAlignment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IconAlignment::Start => write!(f, "start"),
            IconAlignment::Center => write!(f, "center"),
            IconAlignment::End => write!(f, "end"),
        }
    }
}

impl std::str::FromStr for IconAlignment {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "start" => Ok(IconAlignment::Start),
            "center" => Ok(IconAlignment::Center),
            "end" => Ok(IconAlignment::End),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum VisualizationColor {
    #[default]
    SystemAccent, // COSMIC system accent color
    White,
    Black,
    Gray,
    DarkGray,
    Blue,
    DarkBlue,
    Green,
    DarkGreen,
    Orange,
    DarkOrange,
    Purple,
    DarkPurple,
    Red,
    DarkRed,
    Cyan,
    DarkCyan,
    Pink,
    DarkPink,
    Violet,
    DarkViolet,
    PastelBlue,
    PastelGreen,
    PastelOrange,
    PastelPurple,
    PastelRed,
    PastelCyan,
    PastelPink,
    PastelYellow,
    PastelMagenta,
    PastelLavender,
}

impl VisualizationColor {
    /// Human-facing label shown under each colour swatch. UI-only — the enum
    /// persists via serde (variant name), so this is a pretty name, not a wire
    /// id. Matches the `pretty_name()` idiom used by the other theme enums.
    pub fn pretty_name(&self) -> &'static str {
        match self {
            VisualizationColor::SystemAccent => "System Accent",
            VisualizationColor::White => "White",
            VisualizationColor::Black => "Black",
            VisualizationColor::Gray => "Light Gray",
            VisualizationColor::DarkGray => "Dark Gray",
            VisualizationColor::Blue => "Blue",
            VisualizationColor::DarkBlue => "Dark Blue",
            VisualizationColor::Green => "Green",
            VisualizationColor::DarkGreen => "Dark Green",
            VisualizationColor::Orange => "Orange",
            VisualizationColor::DarkOrange => "Dark Orange",
            VisualizationColor::Purple => "Purple",
            VisualizationColor::DarkPurple => "Dark Purple",
            VisualizationColor::Red => "Red",
            VisualizationColor::DarkRed => "Dark Red",
            VisualizationColor::Cyan => "Cyan",
            VisualizationColor::DarkCyan => "Dark Cyan",
            VisualizationColor::Pink => "Pink",
            VisualizationColor::DarkPink => "Dark Pink",
            VisualizationColor::Violet => "Violet",
            VisualizationColor::DarkViolet => "Dark Violet",
            VisualizationColor::PastelBlue => "Pastel Blue",
            VisualizationColor::PastelGreen => "Pastel Green",
            VisualizationColor::PastelOrange => "Pastel Orange",
            VisualizationColor::PastelPurple => "Pastel Purple",
            VisualizationColor::PastelRed => "Pastel Red",
            VisualizationColor::PastelCyan => "Pastel Cyan",
            VisualizationColor::PastelPink => "Pastel Pink",
            VisualizationColor::PastelYellow => "Pastel Yellow",
            VisualizationColor::PastelMagenta => "Pastel Magenta",
            VisualizationColor::PastelLavender => "Pastel Lavender",
        }
    }

    pub fn to_rgb(&self) -> [f32; 3] {
        match self {
            VisualizationColor::SystemAccent => [0.5, 0.5, 0.5],
            VisualizationColor::White => [1.0, 1.0, 1.0],
            VisualizationColor::Black => [0.0, 0.0, 0.0],
            VisualizationColor::Gray => [0.7, 0.7, 0.7],
            VisualizationColor::DarkGray => [0.4, 0.4, 0.4],
            VisualizationColor::Blue => [0.3, 0.65, 1.0],
            VisualizationColor::DarkBlue => [0.15, 0.4, 0.7],
            VisualizationColor::Green => [0.3, 0.8, 0.5],
            VisualizationColor::DarkGreen => [0.15, 0.55, 0.3],
            VisualizationColor::Orange => [1.0, 0.65, 0.3],
            VisualizationColor::DarkOrange => [0.75, 0.4, 0.15],
            VisualizationColor::Purple => [0.85, 0.45, 1.0],
            VisualizationColor::DarkPurple => [0.55, 0.25, 0.75],
            VisualizationColor::Red => [1.0, 0.3, 0.45],
            VisualizationColor::DarkRed => [0.75, 0.15, 0.25],
            VisualizationColor::Cyan => [0.3, 0.85, 0.85],
            VisualizationColor::DarkCyan => [0.15, 0.55, 0.55],
            VisualizationColor::Pink => [1.0, 0.55, 0.65],
            VisualizationColor::DarkPink => [0.75, 0.35, 0.45],
            VisualizationColor::Violet => [0.65, 0.51, 0.95],
            VisualizationColor::DarkViolet => [0.34, 0.23, 0.57],
            VisualizationColor::PastelBlue => [0.68, 0.78, 0.95],
            VisualizationColor::PastelGreen => [0.68, 0.95, 0.78],
            VisualizationColor::PastelOrange => [0.95, 0.82, 0.68],
            VisualizationColor::PastelPurple => [0.92, 0.75, 0.95],
            VisualizationColor::PastelRed => [0.95, 0.68, 0.75],
            VisualizationColor::PastelCyan => [0.68, 0.92, 0.92],
            VisualizationColor::PastelPink => [0.95, 0.78, 0.85],
            VisualizationColor::PastelYellow => [0.95, 0.95, 0.68],
            VisualizationColor::PastelMagenta => [0.95, 0.68, 0.95],
            VisualizationColor::PastelLavender => [0.85, 0.75, 0.95],
        }
    }

    pub fn to_color(&self) -> Color {
        Color::from_rgb(self.to_rgb()[0], self.to_rgb()[1], self.to_rgb()[2])
    }

    /// Convert to Color with access to the COSMIC theme for system accent color
    pub fn to_color_with_theme(&self, cosmic_theme: &cosmic::cosmic_theme::Theme) -> Color {
        match self {
            VisualizationColor::SystemAccent => {
                // Get the accent color from the COSMIC theme
                // Use the base color of the accent component
                let accent_color = cosmic_theme.accent.base.color;
                Color::from_rgb(accent_color.red, accent_color.green, accent_color.blue)
            }
            _ => self.to_color(), // Use existing implementation for other colors
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisualizationColorConfig {
    #[serde(
        default,
        deserialize_with = "super_stt_shared::utils::serde_helpers::deserialize_or_default"
    )]
    pub light_colors: VisualizationColor,
    #[serde(
        default,
        deserialize_with = "super_stt_shared::utils::serde_helpers::deserialize_or_default"
    )]
    pub dark_colors: VisualizationColor,
}

impl Default for VisualizationColorConfig {
    fn default() -> Self {
        Self {
            light_colors: VisualizationColor::SystemAccent,
            dark_colors: VisualizationColor::SystemAccent,
        }
    }
}

impl VisualizationColorConfig {
    pub fn set_color(&mut self, color: VisualizationColor, is_dark: bool) {
        if is_dark {
            self.dark_colors = color;
        } else {
            self.light_colors = color;
        }
    }

    pub fn get_color(&self, is_dark: bool) -> VisualizationColor {
        if is_dark {
            self.dark_colors.clone()
        } else {
            self.light_colors.clone()
        }
    }

    /// Get color as iced Color with theme context for system accent color support
    pub fn get_color_with_theme(
        &self,
        is_dark: bool,
        cosmic_theme: &cosmic::cosmic_theme::Theme,
    ) -> Color {
        let color = self.get_color(is_dark);
        color.to_color_with_theme(cosmic_theme)
    }
}

#[cfg(test)]
mod working_animation_theme_tests {
    use super::WorkingAnimationTheme;

    #[test]
    fn display_from_str_round_trip() {
        for t in [
            WorkingAnimationTheme::Droplet,
            WorkingAnimationTheme::Comet,
            WorkingAnimationTheme::Dots,
        ] {
            assert_eq!(t.to_string().parse::<WorkingAnimationTheme>(), Ok(t));
        }
    }

    #[test]
    fn unknown_is_rejected() {
        assert_eq!("nope".parse::<WorkingAnimationTheme>(), Err(()));
        assert_eq!(
            WorkingAnimationTheme::default(),
            WorkingAnimationTheme::Droplet
        );
    }
}

#[cfg(test)]
mod icon_alignment_tests {
    use super::IconAlignment;

    #[test]
    fn display_from_str_round_trip() {
        for a in [
            IconAlignment::Start,
            IconAlignment::Center,
            IconAlignment::End,
        ] {
            assert_eq!(a.to_string().parse::<IconAlignment>(), Ok(a));
        }
    }

    #[test]
    fn wire_ids_are_snake_case_lowercase() {
        assert_eq!(IconAlignment::Start.to_string(), "start");
        assert_eq!(IconAlignment::Center.to_string(), "center");
        assert_eq!(IconAlignment::End.to_string(), "end");
    }

    #[test]
    fn unknown_is_rejected_and_default_is_end() {
        assert_eq!("middle".parse::<IconAlignment>(), Err(()));
        assert_eq!(IconAlignment::default(), IconAlignment::End);
    }

    #[test]
    fn serde_uses_the_wire_id() {
        // The config persists the lowercase wire id, not the PascalCase variant.
        let json = serde_json::to_string(&IconAlignment::Center).expect("serialize");
        assert_eq!(json, "\"center\"");
        let back: IconAlignment = serde_json::from_str("\"end\"").expect("deserialize");
        assert_eq!(back, IconAlignment::End);
    }
}
