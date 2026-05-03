// SPDX-License-Identifier: GPL-3.0-only
use log::{debug, error, warn};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use super_stt_shared::models::provider::Provider;
use super_stt_shared::models::recording_stop_mode::RecordingStopMode;
use super_stt_shared::models::registry::{self, SourceKind};
use super_stt_shared::models::write_method::WriteMethod;
use super_stt_shared::theme::AudioTheme;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    pub device: DeviceConfig,
    pub audio: AudioConfig,
    pub transcription: TranscriptionConfig,
    #[serde(default)]
    pub online: OnlineConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub preferred_device: String, // "cpu" or "cuda"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    pub theme: AudioTheme,
    #[serde(default = "default_volume")]
    pub volume: u8,
}

fn default_volume() -> u8 {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OnlineConfig {
    /// Whether online models (that send audio to external APIs) are allowed.
    /// Defaults to false for privacy.
    #[serde(default)]
    pub allow_online_models: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionConfig {
    pub preferred_model: String,
    #[serde(default)]
    pub preferred_provider: Provider,
    #[serde(default)]
    pub preferred_source: SourceKind,
    pub write_mode: bool, // Auto-type transcriptions
    #[serde(default)] // For backwards compatibility with existing configs
    pub preview_typing_enabled: bool, // Beta feature: show preview while typing
    #[serde(default)]
    pub recording_stop_mode: RecordingStopMode,
    #[serde(default)]
    pub write_method: WriteMethod,
    #[serde(default)]
    pub custom_models_dir: Option<String>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            device: DeviceConfig {
                preferred_device: "cpu".to_string(), // Default to CPU for compatibility
            },
            audio: AudioConfig {
                theme: AudioTheme::default(),
                volume: default_volume(),
            },
            transcription: TranscriptionConfig {
                preferred_model: registry::default_definition().name.to_string(),
                preferred_provider: registry::default_definition().provider,
                preferred_source: registry::default_definition().source.kind(),
                write_mode: false,             // Default to not auto-typing
                preview_typing_enabled: false, // Default to disabled (beta feature)
                recording_stop_mode: RecordingStopMode::default(),
                write_method: WriteMethod::default(),
                custom_models_dir: None,
            },
            online: OnlineConfig::default(),
        }
    }
}

impl DaemonConfig {
    /// Get the config file path
    fn get_config_path() -> PathBuf {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
                PathBuf::from(home).join(".config")
            })
            .join("super-stt");

        config_dir.join("daemon.toml")
    }

    /// Load configuration from disk.
    ///
    /// Falls back to defaults when the file is missing or cannot be parsed
    /// (e.g. after a format change). When falling back, the default config is
    /// saved to disk so subsequent loads succeed cleanly.
    #[must_use]
    pub fn load() -> Self {
        let config_path = Self::get_config_path();

        match fs::read_to_string(&config_path) {
            Ok(content) => match toml::from_str::<DaemonConfig>(&content) {
                Ok(config) => config,
                Err(e) => {
                    warn!(
                        "Failed to parse config file {}: {e}. Resetting to defaults.",
                        config_path.display()
                    );
                    let config = Self::default();
                    if let Err(save_err) = config.save() {
                        error!("Failed to save default config after parse error: {save_err}");
                    }
                    config
                }
            },
            Err(_) => Self::default(),
        }
    }

    /// Save configuration to disk
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration directory cannot be created,
    /// serialization fails, or the file cannot be written.
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let config_path = Self::get_config_path();

        // Create config directory if it doesn't exist
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let toml_content = toml::to_string_pretty(self)?;
        fs::write(&config_path, toml_content)?;

        debug!("Saved daemon config to {}", config_path.display());
        Ok(())
    }

    /// Update preferred device and save to disk
    pub fn update_preferred_device(&mut self, device: String) {
        self.device.preferred_device = device;
        if let Err(e) = self.save() {
            error!("Failed to save config after device update: {e}");
        }
    }

    /// Update audio theme and save to disk
    pub fn update_audio_theme(&mut self, theme: AudioTheme) {
        self.audio.theme = theme;
        if let Err(e) = self.save() {
            error!("Failed to save config after audio theme update: {e}");
        }
    }

    /// Update preferred model + provider + source and save to disk.
    pub fn update_preferred_model(
        &mut self,
        model: String,
        provider: Provider,
        source: SourceKind,
    ) {
        self.transcription.preferred_model = model;
        self.transcription.preferred_provider = provider;
        self.transcription.preferred_source = source;
        if let Err(e) = self.save() {
            error!("Failed to save config after model update: {e}");
        }
    }

    /// Update write mode and save to disk
    pub fn update_write_mode(&mut self, write_mode: bool) {
        self.transcription.write_mode = write_mode;
        if let Err(e) = self.save() {
            error!("Failed to save config after write mode update: {e}");
        }
    }

    /// Update recording stop mode and save to disk
    pub fn update_recording_stop_mode(&mut self, mode: RecordingStopMode) {
        self.transcription.recording_stop_mode = mode;
        if let Err(e) = self.save() {
            error!("Failed to save config after recording stop mode update: {e}");
        }
    }

    /// Update master volume and save to disk
    pub fn update_volume(&mut self, volume: u8) {
        self.audio.volume = volume;
        if let Err(e) = self.save() {
            error!("Failed to save config after volume update: {e}");
        }
    }

    /// Update allow online models setting and save to disk
    pub fn update_allow_online_models(&mut self, enabled: bool) {
        self.online.allow_online_models = enabled;
        if let Err(e) = self.save() {
            error!("Failed to save config after online models update: {e}");
        }
    }

    /// Update write method and save to disk
    pub fn update_write_method(&mut self, method: WriteMethod) {
        self.transcription.write_method = method;
        if let Err(e) = self.save() {
            error!("Failed to save config after write method update: {e}");
        }
    }

    /// Update custom models directory and save to disk
    pub fn update_custom_models_dir(&mut self, path: Option<String>) {
        self.transcription.custom_models_dir = path;
        if let Err(e) = self.save() {
            error!("Failed to save config after custom models dir update: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_online_models_disabled() {
        let config = DaemonConfig::default();
        assert!(!config.online.allow_online_models);
    }

    #[test]
    fn config_without_online_section_deserializes() {
        let toml_str = r#"
[device]
preferred_device = "cpu"

[audio]
theme = "Classic"
volume = 100

[transcription]
preferred_model = "whisper-tiny"
write_mode = false
preview_typing_enabled = false
recording_stop_mode = "SilenceAndManual"
write_method = "Auto"
"#;
        let config: DaemonConfig = toml::from_str(toml_str).expect("should deserialize");
        assert!(!config.online.allow_online_models);
    }

    #[test]
    fn config_with_online_section_round_trips() {
        let mut config = DaemonConfig::default();
        config.online.allow_online_models = true;

        let toml_str = toml::to_string_pretty(&config).expect("should serialize");
        let parsed: DaemonConfig = toml::from_str(&toml_str).expect("should deserialize");
        assert!(parsed.online.allow_online_models);
    }

    #[test]
    fn config_with_online_model_preferred_round_trips() {
        let mut config = DaemonConfig::default();
        config.online.allow_online_models = true;
        config.transcription.preferred_model = "whisper-1".to_string();

        let toml_str = toml::to_string_pretty(&config).expect("should serialize");
        let parsed: DaemonConfig = toml::from_str(&toml_str).expect("should deserialize");
        assert!(parsed.online.allow_online_models);
        assert_eq!(parsed.transcription.preferred_model, "whisper-1");
    }

    #[test]
    fn config_preserves_all_online_model_variants() {
        for name in [
            "whisper-1",
            "gpt-4o-transcribe",
            "gpt-4o-mini-transcribe",
            "voxtral-mini-transcribe-v2",
            "nova-3",
        ] {
            let model = name.to_string();
            let mut config = DaemonConfig::default();
            config.transcription.preferred_model = model.clone();

            let toml_str = toml::to_string_pretty(&config).expect("should serialize");
            let parsed: DaemonConfig = toml::from_str(&toml_str).expect("should deserialize");
            assert_eq!(parsed.transcription.preferred_model, model);
        }
    }

    #[test]
    fn online_config_default_is_disabled() {
        let online = OnlineConfig::default();
        assert!(!online.allow_online_models);
    }

    #[test]
    fn default_config_has_no_custom_models_dir() {
        let config = DaemonConfig::default();
        assert!(config.transcription.custom_models_dir.is_none());
    }

    #[test]
    fn config_without_custom_models_dir_deserializes() {
        let toml_str = r#"
[device]
preferred_device = "cpu"

[audio]
theme = "Classic"
volume = 100

[transcription]
preferred_model = "whisper-tiny"
write_mode = false
preview_typing_enabled = false
recording_stop_mode = "SilenceAndManual"
write_method = "Auto"
"#;
        let config: DaemonConfig = toml::from_str(toml_str).expect("should deserialize");
        assert!(config.transcription.custom_models_dir.is_none());
    }

    #[test]
    fn config_with_custom_models_dir_round_trips() {
        let mut config = DaemonConfig::default();
        config.transcription.custom_models_dir = Some("/tmp/models".to_string());

        let toml_str = toml::to_string_pretty(&config).expect("should serialize");
        let parsed: DaemonConfig = toml::from_str(&toml_str).expect("should deserialize");
        assert_eq!(
            parsed.transcription.custom_models_dir.as_deref(),
            Some("/tmp/models")
        );
    }

    #[test]
    fn config_with_none_custom_models_dir_round_trips() {
        let config = DaemonConfig::default();

        let toml_str = toml::to_string_pretty(&config).expect("should serialize");
        let parsed: DaemonConfig = toml::from_str(&toml_str).expect("should deserialize");
        assert!(parsed.transcription.custom_models_dir.is_none());
    }
}
