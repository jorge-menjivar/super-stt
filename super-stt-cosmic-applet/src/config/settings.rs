// SPDX-License-Identifier: GPL-3.0-only
use crate::VisualizationSide;
use crate::models::theme::{VisualizationColorConfig, VisualizationTheme, WorkingAnimationTheme};
use log::{debug, error, warn};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppletConfig {
    pub visualization: VisualizationConfig,
    pub ui: UiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisualizationConfig {
    #[serde(
        default,
        deserialize_with = "super_stt_shared::utils::serde_helpers::deserialize_or_default"
    )]
    pub theme: VisualizationTheme,
    // This will be fixed per binary but stored for completeness.
    #[serde(
        default,
        deserialize_with = "super_stt_shared::utils::serde_helpers::deserialize_or_default"
    )]
    pub side: VisualizationSide,
    pub colors: VisualizationColorConfig,
    /// Animation shown while the daemon transcribes (`Processing` state).
    #[serde(
        default,
        deserialize_with = "super_stt_shared::utils::serde_helpers::deserialize_or_default"
    )]
    pub working_animation: WorkingAnimationTheme,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    pub last_popup_state: String, // Store as string for simplicity
    pub show_icon: bool,
    pub icon_alignment: String,
    pub applet_width: u32,        // Width in pixels
    pub show_visualization: bool, // Whether to show visualizations when recording
}

impl Default for AppletConfig {
    fn default() -> Self {
        Self {
            visualization: VisualizationConfig {
                theme: VisualizationTheme::CenteredEqualizer,
                side: VisualizationSide::Full,
                colors: VisualizationColorConfig::default(),
                working_animation: WorkingAnimationTheme::default(),
            },
            ui: UiConfig {
                last_popup_state: "None".to_string(),
                show_icon: true,
                icon_alignment: "end".to_string(),
                applet_width: 120,        // Default width in pixels
                show_visualization: true, // Default to showing visualizations when recording
            },
        }
    }
}

impl AppletConfig {
    /// Get the config file path for a specific applet variant
    fn get_config_path(variant: &str) -> PathBuf {
        super_stt_shared::paths::config_dir().join(format!("applet-{variant}.toml"))
    }

    /// Parse applet config `content`, falling back to defaults on a parse
    /// error. Pure (no I/O) so the load/reset decision is unit-testable.
    /// Returns the config and whether a reset occurred.
    fn parse_or_reset(content: &str) -> (Self, bool) {
        match toml::from_str::<AppletConfig>(content) {
            Ok(config) => (config, false),
            Err(e) => {
                warn!("Failed to parse applet config: {e}. Using defaults.");
                (Self::default(), true)
            }
        }
    }

    /// Load configuration from disk for a specific variant
    pub fn load(variant: &str, vis_side: VisualizationSide) -> Self {
        let config_path = Self::get_config_path(variant);

        let mut config = match fs::read_to_string(&config_path) {
            Ok(content) => Self::parse_or_reset(&content).0,
            Err(_) => Self::default(),
        };
        // Always override the vis_side with the binary-specific value.
        config.visualization.side = vis_side;
        config
    }

    /// Save configuration to disk for a specific variant
    pub fn save(&self, variant: &str) -> Result<(), Box<dyn std::error::Error>> {
        let config_path = Self::get_config_path(variant);

        // Create config directory if it doesn't exist
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let toml_content = toml::to_string_pretty(self)?;
        fs::write(&config_path, toml_content)?;

        debug!("Saved config for {variant} to {}", config_path.display());
        Ok(())
    }

    /// Get the variant name based on `VisualizationSide`
    pub fn get_variant_name(vis_side: &VisualizationSide) -> &'static str {
        match vis_side {
            VisualizationSide::Full => "full",
            VisualizationSide::Left => "left",
            VisualizationSide::Right => "right",
        }
    }

    /// Update visualization theme and save to disk
    pub fn update_visualization_theme(&mut self, theme: VisualizationTheme, variant: &str) {
        self.visualization.theme = theme;
        if let Err(e) = self.save(variant) {
            error!("Failed to save config after visualization theme update: {e}");
        }
    }

    /// Update the working animation theme and save to disk.
    pub fn update_working_animation(&mut self, theme: WorkingAnimationTheme, variant: &str) {
        self.visualization.working_animation = theme;
        if let Err(e) = self.save(variant) {
            error!("Failed to save config after working animation update: {e}");
        }
    }

    /// Update just the applet width and save to disk
    pub fn update_applet_width(&mut self, width: u32, variant: &str) {
        self.ui.applet_width = width;
        if let Err(e) = self.save(variant) {
            error!("Failed to save config after applet width update: {e}");
        }
    }

    /// Update just the icon visibility and save to disk
    pub fn update_show_icon(&mut self, show_icon: bool, variant: &str) {
        self.ui.show_icon = show_icon;
        if let Err(e) = self.save(variant) {
            error!("Failed to save config after icon visibility update: {e}");
        }
    }

    /// Update just the icon alignment and save to disk
    pub fn update_icon_alignment(&mut self, icon_alignment: String, variant: &str) {
        self.ui.icon_alignment = icon_alignment;
        if let Err(e) = self.save(variant) {
            error!("Failed to save config after icon alignment update: {e}");
        }
    }

    /// Update just the show visualization setting and save to disk
    pub fn update_show_visualizations(&mut self, show_visualizations: bool, variant: &str) {
        self.ui.show_visualization = show_visualizations;
        if let Err(e) = self.save(variant) {
            error!("Failed to save config after show visualization update: {e}");
        }
    }

    /// Update visualization colors and save to disk
    pub fn update_visualization_colors(&mut self, colors: VisualizationColorConfig, variant: &str) {
        self.visualization.colors = colors;
        if let Err(e) = self.save(variant) {
            error!("Failed to save config after visualization colors update: {e}");
        }
    }
}

#[cfg(test)]
mod working_animation_config_tests {
    use super::AppletConfig;
    use crate::models::theme::WorkingAnimationTheme;

    #[test]
    fn default_working_animation_is_droplet() {
        assert_eq!(
            AppletConfig::default().visualization.working_animation,
            WorkingAnimationTheme::Droplet
        );
    }

    #[test]
    fn round_trips_through_toml() {
        let mut cfg = AppletConfig::default();
        cfg.visualization.working_animation = WorkingAnimationTheme::Comet;
        let s = toml::to_string_pretty(&cfg).expect("serialize");
        let back: AppletConfig = toml::from_str(&s).expect("deserialize");
        assert_eq!(
            back.visualization.working_animation,
            WorkingAnimationTheme::Comet
        );
    }

    #[test]
    fn old_config_without_field_still_loads() {
        // Build a TOML string from a default config but with the
        // working_animation line removed, simulating a pre-existing config
        // file from before the field existed. It must still parse (serde
        // default) and yield Droplet — not error out and lose settings.
        let full = toml::to_string_pretty(&AppletConfig::default()).expect("serialize");
        let without: String = full
            .lines()
            .filter(|l| !l.trim_start().starts_with("working_animation"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !without.contains("working_animation"),
            "test setup: line not removed"
        );
        let cfg: AppletConfig = toml::from_str(&without).expect("old config must still parse");
        assert_eq!(
            cfg.visualization.working_animation,
            WorkingAnimationTheme::Droplet
        );
    }
}

#[cfg(test)]
mod upgrade_compat_tests {
    use super::AppletConfig;
    use crate::models::theme::{VisualizationColor, VisualizationTheme, WorkingAnimationTheme};

    /// The committed v0.1.3 `applet-full.toml` fixture. The canonical copy lives
    /// in the on-disk corpus so the release gate and these detailed assertions
    /// test the same bytes.
    fn v0_1_3_applet_fixture() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../fixtures/configs/v0.1.3/applet-full.toml");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    #[test]
    fn v0_1_3_full_applet_config_loads() {
        let (cfg, was_reset) = AppletConfig::parse_or_reset(&v0_1_3_applet_fixture());
        assert!(
            !was_reset,
            "a valid v0.1.3 applet config must load, not reset"
        );
        assert_eq!(cfg.visualization.theme, VisualizationTheme::Waveform);
        assert_eq!(
            cfg.visualization.colors.light_colors,
            VisualizationColor::Blue
        );
        assert_eq!(cfg.ui.applet_width, 150);
        assert_eq!(cfg.ui.icon_alignment, "end");
        // New field materializes at its default.
        assert_eq!(
            cfg.visualization.working_animation,
            WorkingAnimationTheme::default()
        );
    }

    #[test]
    fn all_published_applet_configs_load_cleanly() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/configs");
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).expect("fixtures/configs dir must exist") {
            let version_dir = entry.expect("readable dir entry").path();
            if !version_dir.is_dir() {
                continue; // skip README.md and any other non-version files
            }
            for file in std::fs::read_dir(&version_dir).expect("readable version dir") {
                let path = file.expect("readable file entry").path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !(name.starts_with("applet-") && name.ends_with(".toml")) {
                    continue;
                }
                let content = std::fs::read_to_string(&path).expect("read applet fixture");
                let (_, was_reset) = AppletConfig::parse_or_reset(&content);
                assert!(
                    !was_reset,
                    "applet fixture {} must load cleanly (no reset)",
                    path.display()
                );
                checked += 1;
            }
        }
        assert!(
            checked >= 4,
            "expected >= 4 applet fixtures (v0.1.0-v0.1.3), found {checked}"
        );
    }

    #[test]
    fn corrupt_applet_config_resets_to_default() {
        let (cfg, was_reset) = AppletConfig::parse_or_reset("not ::: valid toml [");
        assert!(was_reset, "garbage input must trigger a reset");
        assert_eq!(cfg.visualization.theme, VisualizationTheme::default());
    }

    #[test]
    fn applet_bad_theme_falls_back_preserving_rest() {
        let toml_str = r#"
[visualization]
theme = "Nonexistent"
side = "Full"

[visualization.colors]
light_colors = "Blue"
dark_colors = "PastelGreen"

[ui]
last_popup_state = "None"
show_icon = true
icon_alignment = "end"
applet_width = 150
show_visualization = true
"#;
        let cfg: AppletConfig = toml::from_str(toml_str).expect("must parse, not error");
        assert_eq!(cfg.visualization.theme, VisualizationTheme::default()); // reset
        assert_eq!(
            cfg.visualization.colors.light_colors,
            VisualizationColor::Blue
        ); // preserved
        assert_eq!(cfg.ui.applet_width, 150); // preserved
        assert!(cfg.ui.show_icon);
    }
}
