// SPDX-License-Identifier: GPL-3.0-only
use log::{debug, error, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use super_stt_shared::models::notification_method::NotificationMethod;
use super_stt_shared::models::recording_stop_mode::RecordingStopMode;
use super_stt_shared::models::update_beta_optin::UpdateBetaOptIn;
use super_stt_shared::models::write_method::WriteMethod;
use super_stt_shared::theme::AudioTheme;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    pub device: DeviceConfig,
    pub audio: AudioConfig,
    pub transcription: TranscriptionConfig,
    #[serde(default)]
    pub online: OnlineConfig,
    #[serde(default)]
    pub backends: BackendsConfig,
    #[serde(default)]
    pub update: UpdateConfig,
    #[serde(default)]
    pub post_processor: PostProcessorConfig,
}

/// Self-update checking. Contract: docs/protocol/endpoints/v1/update.md
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateConfig {
    /// Periodic background checks + desktop notification. `POST
    /// /v1/update/check` works regardless of this flag.
    #[serde(default = "default_check_enabled")]
    pub check_enabled: bool,
    /// An unparseable stored value degrades to the default rather than
    /// failing the whole config load.
    #[serde(
        default,
        deserialize_with = "super_stt_shared::utils::serde_helpers::deserialize_or_default"
    )]
    pub beta_optin: UpdateBetaOptIn,
}

fn default_check_enabled() -> bool {
    true
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            check_enabled: true,
            beta_optin: UpdateBetaOptIn::default(),
        }
    }
}

/// User-set configuration for installed backends. Secrets live in the keyring;
/// only non-sensitive **options** are stored here (plaintext).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BackendsConfig {
    /// Per-backend option overrides: backend `source` → (option name → value).
    /// An absent entry means "use the manifest default".
    #[serde(default)]
    pub options: HashMap<String, HashMap<String, String>>,
    /// Per-model settings: backend `source` → (model name → settings).
    #[serde(default)]
    pub models: HashMap<String, HashMap<String, ModelSettings>>,
}

/// Per-model configuration. A struct (not a bare value) so future per-model
/// settings have a home.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelSettings {
    /// Per-model language override: a BCP-47 tag, `"auto"`, or `None`
    /// (Automatic — inherit the global `primary_language`, else the model's primary).
    #[serde(default)]
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub preferred_device: String, // "cpu" or "gpu"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    #[serde(
        default,
        deserialize_with = "super_stt_shared::utils::serde_helpers::deserialize_or_default"
    )]
    pub theme: AudioTheme,
    #[serde(default = "default_volume")]
    pub volume: u8,
}

fn default_volume() -> u8 {
    100
}

/// The transcript post-processor: a second model, selected independently of
/// the transcription model, that rewrites each final transcript before the
/// daemon types or returns it.
///
/// Contract: `docs/protocol/endpoints/v1/pipeline.md` (stage 2). Every field is
/// defaulted, so a `daemon.toml` written before the section existed loads with
/// post-processing off.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PostProcessorConfig {
    /// Whether final transcripts are run through the selected post-processor.
    /// Off by default: post-processing costs latency, and a cloud processor
    /// would send the transcript text to a third party.
    #[serde(default)]
    pub enabled: bool,
    /// Wire name of the selected post-processor model. Empty when none is
    /// selected.
    #[serde(default)]
    pub model: String,
    /// Repo id of the backend serving `model`. Empty when none is selected.
    /// Held with `model` as the `(name, source)` identity pair every other
    /// model selection uses.
    #[serde(default)]
    pub source: String,
    /// The stage's own device preference, `cpu` or `gpu`. The post-processor
    /// runs beside the transcription model, so it gets hardware chosen for it
    /// rather than stage 1's. Empty — every config written before the field
    /// existed — means "follow the transcription preference", which is what
    /// those loads always did.
    #[serde(default)]
    pub device: String,
}

impl PostProcessorConfig {
    /// The selected `(model, source)` pair, or `None` when the selection is
    /// incomplete. Both halves are required: a name without the backend that
    /// serves it resolves to nothing.
    #[must_use]
    pub fn selection(&self) -> Option<(&str, &str)> {
        (!self.model.is_empty() && !self.source.is_empty())
            .then_some((self.model.as_str(), self.source.as_str()))
    }

    /// Whether post-processing should actually run: enabled *and* pointed at a
    /// model. The two are stored separately so toggling the feature off does
    /// not discard the user's choice.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.enabled && self.selection().is_some()
    }
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
    /// Compatibility shim; the daemon resolves its startup model by
    /// `(preferred_model, preferred_source)` and never reads this.
    ///
    /// It is written, not merely preserved, because `daemon.toml` outlives the
    /// binary that wrote it. Daemons through v0.2.0 resolve their startup model
    /// by `(model, provider, source)`, so a user who rolls back after a bad
    /// upgrade gets an idle daemon — with no error, just a lost selection — if
    /// this key is missing *or* stale. Keeping it in step with the selected
    /// model is what makes the rollback recoverable.
    ///
    /// Delete once no supported daemon resolves by provider.
    #[serde(default)]
    pub preferred_provider: String,
    #[serde(default)]
    pub preferred_source: String,
    #[serde(default)]
    pub write_mode: bool, // Auto-type transcriptions
    #[serde(default)] // For backwards compatibility with existing configs
    pub preview_typing_enabled: bool, // Beta feature: show preview while typing
    #[serde(
        default,
        deserialize_with = "super_stt_shared::utils::serde_helpers::deserialize_or_default"
    )]
    pub recording_stop_mode: RecordingStopMode,
    #[serde(
        default,
        deserialize_with = "super_stt_shared::utils::serde_helpers::deserialize_or_default"
    )]
    pub write_method: WriteMethod,
    /// How a recording failure is surfaced to the user. An unparseable stored
    /// value degrades to the default rather than failing the whole config load.
    #[serde(
        default,
        deserialize_with = "super_stt_shared::utils::serde_helpers::deserialize_or_default"
    )]
    pub notification_method: NotificationMethod,
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
    /// Global default transcription language: a BCP-47 tag, the reserved
    /// `"auto"`, or `None` (no preference; models use their `primary_language`).
    #[serde(default)]
    pub primary_language: Option<String>,
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
                // Empty preference: the daemon stays idle until a model is
                // selected — it never auto-picks one, since loading a model can
                // pull gigabytes.
                preferred_model: String::new(),
                preferred_provider: String::new(),
                preferred_source: String::new(),
                write_mode: false,             // Default to not auto-typing
                preview_typing_enabled: false, // Default to disabled (beta feature)
                recording_stop_mode: RecordingStopMode::default(),
                write_method: WriteMethod::default(),
                notification_method: NotificationMethod::default(),
                custom_models_dir: None,
                backends_dir: None,
                active_backend: None,
                primary_language: None,
            },
            online: OnlineConfig::default(),
            backends: BackendsConfig::default(),
            update: UpdateConfig::default(),
            post_processor: PostProcessorConfig::default(),
        }
    }
}

/// The `daemon.toml` path used by `DaemonConfig::get_config_path()` under
/// `#[cfg(test)]`: a directory under the OS temp dir, unique per test
/// *process* (keyed by pid) and cached for the life of that process so every
/// test in the binary agrees on the same path. This is what keeps unit tests
/// that call `load()`/`save()` off the developer's real config file.
#[cfg(test)]
fn test_config_path() -> PathBuf {
    use std::sync::OnceLock;
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let dir =
            std::env::temp_dir().join(format!("super-stt-test-config-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        dir.join("daemon.toml")
    })
    .clone()
}

impl DaemonConfig {
    /// Get the config file path.
    ///
    /// Under `#[cfg(test)]` this resolves to a process-local temp file instead
    /// of the real XDG config path, so `load()`/`save()` in this crate's unit
    /// tests can never read or clobber the developer's real
    /// `~/.config/super-stt/daemon.toml`. Known limitation: this only covers
    /// unit tests compiled *into* this crate (`cfg(test)`). Integration tests
    /// under `tests/` link against the crate without `cfg(test)`, so they
    /// still resolve the real path — not addressed here.
    fn get_config_path() -> PathBuf {
        #[cfg(test)]
        {
            test_config_path()
        }
        #[cfg(not(test))]
        {
            super_stt_shared::paths::config_dir().join("daemon.toml")
        }
    }

    /// Parse config file `content` into a [`DaemonConfig`], falling back to
    /// defaults on a parse error. Pure (no I/O) so the load/reset decision is
    /// unit-testable without touching the real config path. Returns the config
    /// and whether a reset occurred (so the caller knows to persist defaults).
    ///
    /// `preferred_device` is a bare `String`, not an enum, so no
    /// `deserialize_or_default` catches a stale value at the serde layer —
    /// it is normalized here instead. Unlike `POST /active_device`, which
    /// rejects an unparseable value outright, a persisted value has no such
    /// option: a daemon that refused to start over a stale config field would
    /// be worse than one that falls back to the default, so this degrades to
    /// `"cpu"` rather than triggering a full reset. The deprecated `cuda`/
    /// `metal` spellings normalize to `gpu` rather than falling back, the same
    /// as the wire setter, so a config written before this vocabulary still
    /// loads onto the accelerator it always meant.
    fn parse_or_reset(content: &str) -> (Self, bool) {
        match toml::from_str::<DaemonConfig>(content) {
            Ok(mut config) => {
                config.device.preferred_device =
                    crate::daemon::device_management::parse_device_preference(
                        &config.device.preferred_device,
                    )
                    .unwrap_or_else(|| "cpu".to_string());
                (config, false)
            }
            Err(e) => {
                warn!("Failed to parse config: {e}. Resetting to defaults.");
                (Self::default(), true)
            }
        }
    }

    /// Load configuration from disk.
    ///
    /// Falls back to defaults when the file is missing or cannot be parsed
    /// (e.g. after a format change). When falling back, the default config is
    /// saved to disk so subsequent loads succeed cleanly. If individual fields
    /// fell back to their defaults (a stale enum value via the shared
    /// `deserialize_or_default` helper), the canonical form is rewritten so the
    /// warning doesn't repeat next startup.
    #[must_use]
    pub fn load() -> Self {
        let config_path = Self::get_config_path();

        let Ok(content) = fs::read_to_string(&config_path) else {
            return Self::default();
        };

        let (config, was_reset) = Self::parse_or_reset(&content);
        if was_reset {
            // Persist the regenerated defaults so subsequent loads are clean.
            if let Err(e) = config.save() {
                error!("Failed to save default config after parse error: {e}");
            }
        } else if let Ok(canonical) = toml::to_string_pretty(&config)
            && canonical != content
            && let Err(e) = config.save()
        {
            error!("Failed to rewrite config in canonical form: {e}");
        }
        config
    }

    /// Save configuration to disk. Blocking (`std::fs::write`); on the async
    /// runtime call it via `persist_config()` / `spawn_blocking`, not inline.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration directory cannot be created,
    /// serialization fails, or the file cannot be written.
    pub fn save(&self) -> anyhow::Result<()> {
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

    // Config mutators are PURE (Tier 3 #3): they mutate in-memory state only.
    // The caller persists once via `persist_config()` (which writes off the
    // async runtime), so there are no blocking `fs::write`s under the config
    // lock and no double writes.

    /// Update preferred device.
    pub fn update_preferred_device(&mut self, device: String) {
        self.device.preferred_device = device;
    }

    /// Update audio theme.
    pub fn update_audio_theme(&mut self, theme: AudioTheme) {
        self.audio.theme = theme;
    }

    /// Update preferred model + source, and the legacy `preferred_provider`
    /// the model declares (see [`TranscriptionConfig::preferred_provider`] for
    /// why a stale value is as bad as a missing one).
    pub fn update_preferred_model(
        &mut self,
        model: String,
        source: String,
        provider: Option<String>,
    ) {
        self.transcription.preferred_model = model;
        self.transcription.preferred_source = source;
        self.transcription.preferred_provider = provider.unwrap_or_default();
    }

    /// Clear the loaded-model preference (model + source) while keeping the
    /// active backend selected. Used by the unload path so a daemon restart
    /// stays idle instead of reloading the unloaded model.
    pub fn clear_preferred_model(&mut self) {
        self.transcription.preferred_model = String::new();
        self.transcription.preferred_source = String::new();
        self.transcription.preferred_provider = String::new();
    }

    /// Set the active backend (its relative install dir) and drop the loaded
    /// model preference — selecting a backend does not load a model.
    pub fn update_active_backend(&mut self, dir: String) {
        self.transcription.active_backend = Some(dir);
        self.transcription.preferred_model = String::new();
        self.transcription.preferred_provider = String::new();
    }

    /// Repoint the active backend to `new_dir` without touching the loaded
    /// model preference.
    ///
    /// Deliberately narrower than [`update_active_backend`]: that method
    /// clears `preferred_model`/`preferred_provider` because it models the
    /// user *choosing a different backend*, where any previously loaded
    /// model no longer applies. This method models an install-time directory
    /// rename of the *same* backend — its identity, models, and selection are
    /// unchanged, only the on-disk directory name moved. Using
    /// `update_active_backend` here would silently wipe the user's model
    /// choice as a side effect of an update. Do not merge the two.
    pub fn rename_active_backend(&mut self, new_dir: String) {
        self.transcription.active_backend = Some(new_dir);
    }

    /// Clear the active backend and the loaded-model preference (→ idle).
    pub fn clear_active_backend(&mut self) {
        self.transcription.active_backend = None;
        self.transcription.preferred_model = String::new();
        self.transcription.preferred_source = String::new();
        self.transcription.preferred_provider = String::new();
    }

    /// Select the post-processor model and whether it runs. Stored as the same
    /// `(name, source)` pair every model selection uses.
    pub fn enable_post_processor(&mut self, model: String, source: String, device: Option<String>) {
        self.post_processor.enabled = true;
        self.post_processor.model = model;
        self.post_processor.source = source;
        if let Some(device) = device {
            self.post_processor.device = device;
        }
    }

    /// Stop running the post-processor, keeping the selection so re-enabling
    /// is one call. The counterpart of `DELETE /active_model`, which unloads
    /// the model but leaves its backend selected.
    pub fn disable_post_processor(&mut self) {
        self.post_processor.enabled = false;
    }

    /// Select the backend that provides the post-processor.
    ///
    /// Switching to a *different* backend drops the model with it: the name
    /// belonged to the old backend and means nothing under the new one. This
    /// mirrors `POST /active_backend`, which unloads the current model when the
    /// backend changes.
    pub fn select_post_processor_backend(&mut self, source: String) {
        if self.post_processor.source != source {
            self.post_processor.model.clear();
            self.post_processor.enabled = false;
        }
        self.post_processor.source = source;
    }

    /// Deselect the backend and forget everything with it (→ nothing loaded).
    pub fn clear_post_processor_backend(&mut self) {
        self.post_processor = PostProcessorConfig::default();
    }

    /// Update master volume.
    pub fn update_volume(&mut self, volume: u8) {
        self.audio.volume = volume;
    }

    /// Set or clear a backend option override. An empty `value` clears the
    /// override (the backend falls back to its manifest default).
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

    pub fn update_primary_language(&mut self, language: Option<String>) {
        self.transcription.primary_language = language;
    }

    #[must_use]
    pub fn primary_language(&self) -> Option<&str> {
        self.transcription.primary_language.as_deref()
    }

    /// Set (`Some`) or clear (`None`) a per-model language override.
    pub fn update_model_language(
        &mut self,
        source: String,
        model: String,
        language: Option<String>,
    ) {
        match language {
            Some(v) => {
                self.backends
                    .models
                    .entry(source)
                    .or_default()
                    .entry(model)
                    .or_default()
                    .language = Some(v);
            }
            None => {
                if let Some(models) = self.backends.models.get_mut(&source)
                    && let Some(settings) = models.get_mut(&model)
                {
                    settings.language = None;
                }
            }
        }
    }

    #[must_use]
    pub fn model_language(&self, source: &str, model: &str) -> Option<&str> {
        self.backends
            .models
            .get(source)
            .and_then(|m| m.get(model))
            .and_then(|s| s.language.as_deref())
    }
}

#[cfg(test)]
mod language_config_tests {
    use super::*;

    #[test]
    fn primary_and_model_language_round_trip_through_toml() {
        let mut cfg = DaemonConfig::default();
        assert_eq!(cfg.primary_language(), None);

        cfg.update_primary_language(Some("es-MX".to_string()));
        cfg.update_model_language(
            "github.com/x/whisper".to_string(),
            "whisper-large".to_string(),
            Some("fr".to_string()),
        );

        let toml = toml::to_string(&cfg).expect("serialize");
        let back: DaemonConfig = toml::from_str(&toml).expect("deserialize");

        assert_eq!(back.primary_language(), Some("es-MX"));
        assert_eq!(
            back.model_language("github.com/x/whisper", "whisper-large"),
            Some("fr")
        );
        assert_eq!(back.model_language("github.com/x/whisper", "absent"), None);
    }

    #[test]
    fn clearing_model_language_sets_none() {
        let mut cfg = DaemonConfig::default();
        cfg.update_model_language("s".into(), "m".into(), Some("de".into()));
        cfg.update_model_language("s".into(), "m".into(), None);
        assert_eq!(cfg.model_language("s", "m"), None);
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
