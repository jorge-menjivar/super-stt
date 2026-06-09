// SPDX-License-Identifier: GPL-3.0-only
use log::{debug, error, warn};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use super_stt_shared::models::provider::Provider;
use super_stt_shared::models::recording_stop_mode::RecordingStopMode;
use super_stt_shared::models::write_method::WriteMethod;
use super_stt_shared::theme::AudioTheme;

/// Deserialize a value via its serde impl, falling back to `Default` if the
/// stored representation is no longer recognized (e.g. a TOML field saved by
/// an older build using a stale format). Logs a warning so the migration is
/// observable. The model loader has its own fallback for invalid (model,
/// provider) combinations, so a single bad field shouldn't fail the whole
/// config.
fn deserialize_or_default<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    T: Default + serde::de::DeserializeOwned,
    D: Deserializer<'de>,
{
    let raw = serde_json::Value::deserialize(deserializer)?;
    match serde_json::from_value::<T>(raw.clone()) {
        Ok(value) => Ok(value),
        Err(e) => {
            warn!("config field {raw} unrecognized ({e}); using default");
            Ok(T::default())
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    pub device: DeviceConfig,
    pub audio: AudioConfig,
    pub transcription: TranscriptionConfig,
    #[serde(default)]
    pub online: OnlineConfig,
    #[serde(default)]
    pub backends: BackendsConfig,
}

/// User-set configuration for installed backends. Secrets live in the keyring;
/// only non-sensitive **options** are stored here (plaintext).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BackendsConfig {
    /// Per-backend option overrides: backend `source` → (option name → value).
    /// An absent entry means "use the manifest default".
    #[serde(default)]
    pub options: HashMap<String, HashMap<String, String>>,
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
    #[serde(default)]
    pub preferred_model: String,
    #[serde(default, deserialize_with = "deserialize_or_default")]
    pub preferred_provider: Provider,
    /// Repo id of the backend that serves the preferred model. Empty means the
    /// daemon picks the first installed backend serving `(model, provider)`.
    #[serde(default)]
    pub preferred_source: String,
    #[serde(default)]
    pub write_mode: bool, // Auto-type transcriptions
    #[serde(default)] // For backwards compatibility with existing configs
    pub preview_typing_enabled: bool, // Beta feature: show preview while typing
    #[serde(default)]
    pub recording_stop_mode: RecordingStopMode,
    #[serde(default)]
    pub write_method: WriteMethod,
    /// Vestigial: retained for config compatibility. Custom models are now
    /// provided as backends discovered under [`backends_dir`].
    #[serde(default)]
    pub custom_models_dir: Option<String>,
    /// Directory scanned for installed backends. `None` uses the default
    /// (`<data_dir>/super-stt/backends`).
    #[serde(default)]
    pub backends_dir: Option<String>,
    /// Relative install dir (subdir of [`backends_dir`]) of the selected active
    /// backend, or `None` when idle. Metadata (name/source/models) is read from
    /// that dir's `backend.toml`. An active backend with an empty
    /// `preferred_model` means "backend selected, no model loaded".
    #[serde(default)]
    pub active_backend: Option<String>,
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
                // Empty preference: the daemon loads the first usable model from
                // whatever backends are discovered on disk.
                preferred_model: String::new(),
                preferred_provider: Provider::default(),
                preferred_source: String::new(),
                write_mode: false,             // Default to not auto-typing
                preview_typing_enabled: false, // Default to disabled (beta feature)
                recording_stop_mode: RecordingStopMode::default(),
                write_method: WriteMethod::default(),
                custom_models_dir: None,
                backends_dir: None,
                active_backend: None,
            },
            online: OnlineConfig::default(),
            backends: BackendsConfig::default(),
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
    /// saved to disk so subsequent loads succeed cleanly. If individual
    /// fields fell back via [`deserialize_or_default`], the canonical form
    /// is rewritten so the warning doesn't repeat next startup.
    #[must_use]
    pub fn load() -> Self {
        let config_path = Self::get_config_path();

        match fs::read_to_string(&config_path) {
            Ok(content) => match toml::from_str::<DaemonConfig>(&content) {
                Ok(config) => {
                    if let Ok(canonical) = toml::to_string_pretty(&config)
                        && canonical != content
                        && let Err(e) = config.save()
                    {
                        error!("Failed to rewrite config in canonical form: {e}");
                    }
                    config
                }
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
    pub fn update_preferred_model(&mut self, model: String, provider: Provider, source: String) {
        self.transcription.preferred_model = model;
        self.transcription.preferred_provider = provider;
        self.transcription.preferred_source = source;
        if let Err(e) = self.save() {
            error!("Failed to save config after model update: {e}");
        }
    }

    /// Set the active backend (its relative install dir) and drop the loaded
    /// model preference — selecting a backend does not load a model. Saves.
    pub fn update_active_backend(&mut self, dir: String) {
        self.transcription.active_backend = Some(dir);
        self.transcription.preferred_model = String::new();
        if let Err(e) = self.save() {
            error!("Failed to save config after active backend update: {e}");
        }
    }

    /// Clear the active backend and the loaded-model preference (→ idle). Saves.
    pub fn clear_active_backend(&mut self) {
        self.transcription.active_backend = None;
        self.transcription.preferred_model = String::new();
        self.transcription.preferred_source = String::new();
        if let Err(e) = self.save() {
            error!("Failed to save config after clearing active backend: {e}");
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

    /// Set or clear a backend option override and save to disk. An empty
    /// `value` clears the override (the backend falls back to its manifest
    /// default).
    pub fn update_backend_option(&mut self, source: String, name: String, value: String) {
        if value.is_empty() {
            if let Some(opts) = self.backends.options.get_mut(&source) {
                opts.remove(&name);
                if opts.is_empty() {
                    self.backends.options.remove(&source);
                }
            }
        } else {
            self.backends
                .options
                .entry(source)
                .or_default()
                .insert(name, value);
        }
        if let Err(e) = self.save() {
            error!("Failed to save config after backend option update: {e}");
        }
    }

    /// The configured override for a backend option, if any.
    #[must_use]
    pub fn backend_option(&self, source: &str, name: &str) -> Option<&str> {
        self.backends
            .options
            .get(source)
            .and_then(|opts| opts.get(name))
            .map(String::as_str)
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
