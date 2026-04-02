// SPDX-License-Identifier: GPL-3.0-only
use crate::VisualizationSide;
use crate::models::theme::{VisualizationColorConfig, VisualizationTheme};
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
    pub theme: VisualizationTheme,
    pub side: VisualizationSide, // This will be fixed per binary but stored for completeness
    pub colors: VisualizationColorConfig,
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
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
                PathBuf::from(home).join(".config")
            })
            .join("super-stt");

        config_dir.join(format!("applet-{variant}.toml"))
    }

    /// Load configuration from disk for a specific variant
    pub fn load(variant: &str, vis_side: VisualizationSide) -> Self {
        let config_path = Self::get_config_path(variant);

        if let Ok(content) = fs::read_to_string(&config_path) {
            match toml::from_str::<AppletConfig>(&content) {
                Ok(mut config) => {
                    // Always override the vis_side with the binary-specific value
                    config.visualization.side = vis_side;
                    config
                }
                Err(e) => {
                    warn!(
                        "Failed to parse config file {}: {e}. Using defaults.",
                        config_path.display()
                    );
                    let mut config = Self::default();
                    config.visualization.side = vis_side;
                    config
                }
            }
        } else {
            let mut config = Self::default();
            config.visualization.side = vis_side;
            config
        }
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
