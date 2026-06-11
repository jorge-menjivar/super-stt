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

impl VisualizationTheme {
    pub fn from_str(s: &str) -> Self {
        match s {
            "pulse" => VisualizationTheme::Pulse,
            "b_equalizer" => VisualizationTheme::BottomEqualizer,
            "c_equalizer" => VisualizationTheme::CenteredEqualizer,
            "waveform" => VisualizationTheme::Waveform,
            _ => VisualizationTheme::default(),
        }
    }

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

impl WorkingAnimationTheme {
    pub fn from_str(s: &str) -> Self {
        match s {
            "droplet" => WorkingAnimationTheme::Droplet,
            "comet" => WorkingAnimationTheme::Comet,
            "dots" => WorkingAnimationTheme::Dots,
            _ => WorkingAnimationTheme::default(),
        }
    }

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VisualizationColor {
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

impl std::fmt::Display for VisualizationColor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VisualizationColor::SystemAccent => write!(f, "System Accent"),
            VisualizationColor::White => write!(f, "White"),
            VisualizationColor::Black => write!(f, "Black"),
            VisualizationColor::Gray => write!(f, "Light Gray"),
            VisualizationColor::DarkGray => write!(f, "Dark Gray"),
            VisualizationColor::Blue => write!(f, "Blue"),
            VisualizationColor::DarkBlue => write!(f, "Dark Blue"),
            VisualizationColor::Green => write!(f, "Green"),
            VisualizationColor::DarkGreen => write!(f, "Dark Green"),
            VisualizationColor::Orange => write!(f, "Orange"),
            VisualizationColor::DarkOrange => write!(f, "Dark Orange"),
            VisualizationColor::Purple => write!(f, "Purple"),
            VisualizationColor::DarkPurple => write!(f, "Dark Purple"),
            VisualizationColor::Red => write!(f, "Red"),
            VisualizationColor::DarkRed => write!(f, "Dark Red"),
            VisualizationColor::Cyan => write!(f, "Cyan"),
            VisualizationColor::DarkCyan => write!(f, "Dark Cyan"),
            VisualizationColor::Pink => write!(f, "Pink"),
            VisualizationColor::DarkPink => write!(f, "Dark Pink"),
            VisualizationColor::Violet => write!(f, "Violet"),
            VisualizationColor::DarkViolet => write!(f, "Dark Violet"),
            VisualizationColor::PastelBlue => write!(f, "Pastel Blue"),
            VisualizationColor::PastelGreen => write!(f, "Pastel Green"),
            VisualizationColor::PastelOrange => write!(f, "Pastel Orange"),
            VisualizationColor::PastelPurple => write!(f, "Pastel Purple"),
            VisualizationColor::PastelRed => write!(f, "Pastel Red"),
            VisualizationColor::PastelCyan => write!(f, "Pastel Cyan"),
            VisualizationColor::PastelPink => write!(f, "Pastel Pink"),
            VisualizationColor::PastelYellow => write!(f, "Pastel Yellow"),
            VisualizationColor::PastelMagenta => write!(f, "Pastel Magenta"),
            VisualizationColor::PastelLavender => write!(f, "Pastel Lavender"),
        }
    }
}

impl From<std::string::String> for VisualizationColor {
    fn from(input: String) -> Self {
        match input.as_str() {
            "white" => VisualizationColor::White,
            "black" => VisualizationColor::Black,
            "gray" => VisualizationColor::Gray,
            "dark_gray" => VisualizationColor::DarkGray,
            "blue" => VisualizationColor::Blue,
            "dark_blue" => VisualizationColor::DarkBlue,
            "green" => VisualizationColor::Green,
            "dark_green" => VisualizationColor::DarkGreen,
            "orange" => VisualizationColor::Orange,
            "dark_orange" => VisualizationColor::DarkOrange,
            "purple" => VisualizationColor::Purple,
            "dark_purple" => VisualizationColor::DarkPurple,
            "red" => VisualizationColor::Red,
            "dark_red" => VisualizationColor::DarkRed,
            "cyan" => VisualizationColor::Cyan,
            "dark_cyan" => VisualizationColor::DarkCyan,
            "pink" => VisualizationColor::Pink,
            "dark_pink" => VisualizationColor::DarkPink,
            "violet" => VisualizationColor::Violet,
            "dark_violet" => VisualizationColor::DarkViolet,
            "pastel_blue" => VisualizationColor::PastelBlue,
            "pastel_green" => VisualizationColor::PastelGreen,
            "pastel_orange" => VisualizationColor::PastelOrange,
            "pastel_purple" => VisualizationColor::PastelPurple,
            "pastel_red" => VisualizationColor::PastelRed,
            "pastel_cyan" => VisualizationColor::PastelCyan,
            "pastel_pink" => VisualizationColor::PastelPink,
            "pastel_yellow" => VisualizationColor::PastelYellow,
            "pastel_magenta" => VisualizationColor::PastelMagenta,
            "pastel_lavender" => VisualizationColor::PastelLavender,
            _ => VisualizationColor::SystemAccent,
        }
    }
}

impl VisualizationColor {
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
    pub light_colors: VisualizationColor,
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

#[derive(Debug, Clone, Default)]
pub struct ThemeConfig {
    pub visualization_theme: VisualizationTheme,
    pub visualization_color_config: VisualizationColorConfig,
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
            assert_eq!(WorkingAnimationTheme::from_str(&t.to_string()), t);
        }
    }

    #[test]
    fn unknown_falls_back_to_default() {
        assert_eq!(
            WorkingAnimationTheme::from_str("nope"),
            WorkingAnimationTheme::default()
        );
        assert_eq!(
            WorkingAnimationTheme::default(),
            WorkingAnimationTheme::Droplet
        );
    }
}
