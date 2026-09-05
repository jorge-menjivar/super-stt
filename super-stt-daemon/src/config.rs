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
    /// The device this model loads on: `"cpu"` or `"gpu"`, or `None` to use
    /// [`DeviceConfig::preferred_device`]. A device belongs to a model
    /// because it is a property of the model: a small one runs fine on the
    /// CPU while the large one beside it needs the GPU, and a post-processor
    /// sharing the pipeline with either has its own answer again.
    #[serde(default)]
    pub device: Option<String>,
}

/// The device models fall back to when they have none of their own
/// ([`ModelSettings::device`]). Not settable over the API any more — every
/// setter is per model — but kept in the file so a config written when the
/// preference was global keeps loading models where it always did.
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

/// The `enabled` a config predating the field is read with. See
/// [`TranscriptionConfig::enabled`] for why it is `true` rather than the
/// struct's own default.
fn default_migrated_enabled() -> bool {
    true
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionConfig {
    /// Whether the user has stage 1 switched on: what Load sets and Unload
    /// clears. Held separately from `preferred_model` so an unload can stay
    /// idle across a restart *without* discarding the selection — which is how
    /// [`PostProcessorConfig::enabled`] has always worked, and which stage 1
    /// lacked until the stages were made to behave alike.
    ///
    /// Defaults to `true` when the key is absent, which is the only thing that
    /// default is for: a `daemon.toml` written before this field existed loaded
    /// its `preferred_model` on sight, so reading a missing key as `false`
    /// would silently idle every upgraded daemon. A config with no model
    /// selected is inert either way — see [`TranscriptionConfig::is_active`] —
    /// so there is no case where the migration default is wrong. A *new* config
    /// starts `false`, because nothing is selected yet.
    #[serde(default = "default_migrated_enabled")]
    pub enabled: bool,
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

impl TranscriptionConfig {
    /// The selected `(model, source)` pair, or `None` when the selection is
    /// incomplete. Deliberately the same shape as
    /// [`PostProcessorConfig::selection`]: both halves are required, since a
    /// name without the backend that serves it resolves to nothing.
    #[must_use]
    pub fn selection(&self) -> Option<(&str, &str)> {
        (!self.preferred_model.is_empty() && !self.preferred_source.is_empty()).then_some((
            self.preferred_model.as_str(),
            self.preferred_source.as_str(),
        ))
    }

    /// Whether stage 1 should actually be running: switched on *and* pointed at
    /// a model. The twin of [`PostProcessorConfig::is_active`], and the value
    /// the stage reports as `enabled` — a stage switched on with nothing
    /// selected is not running, and a card told otherwise would offer to unload
    /// a model that does not exist.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.enabled && self.selection().is_some()
    }
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
                // Nothing is selected yet, so the stage is not switched on.
                // Distinct from the serde default above, which answers a
                // different question: what an *older* config meant.
                enabled: false,
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
    /// Devices are bare `String`s, not an enum, so no `deserialize_or_default`
    /// catches a stale value at the serde layer — they are normalized here
    /// instead. Unlike the wire setter, which rejects an unparseable value
    /// outright, a persisted value has no such option: a daemon that refused
    /// to start over a stale config field would be worse than one that falls
    /// back to the default, so the global default degrades to `"cpu"` and a
    /// per-model device to "none of its own" rather than triggering a full
    /// reset. The deprecated `cuda`/`metal` spellings normalize to `gpu`
    /// rather than falling back, the same as the wire setter, so a config
    /// written before this vocabulary still loads onto the accelerator it
    /// always meant.
    fn parse_or_reset(content: &str) -> (Self, bool) {
        use crate::daemon::device_management::parse_device_preference;
        match toml::from_str::<DaemonConfig>(content) {
            Ok(mut config) => {
                config.device.preferred_device =
                    parse_device_preference(&config.device.preferred_device)
                        .unwrap_or_else(|| "cpu".to_string());
                for settings in config
                    .backends
                    .models
                    .values_mut()
                    .flat_map(HashMap::values_mut)
                {
                    settings.device = settings.device.as_deref().and_then(parse_device_preference);
                }
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

    /// Set (`Some`) or clear (`None`) the device a model loads on. The value
    /// is the normalized `cpu`/`gpu` preference; callers validate before
    /// storing.
    pub fn update_model_device(&mut self, source: &str, model: &str, device: Option<String>) {
        match device {
            Some(v) => {
                self.backends
                    .models
                    .entry(source.to_string())
                    .or_default()
                    .entry(model.to_string())
                    .or_default()
                    .device = Some(v);
            }
            None => {
                if let Some(models) = self.backends.models.get_mut(source)
                    && let Some(settings) = models.get_mut(model)
                {
                    settings.device = None;
                }
            }
        }
    }

    /// A model's own device, if it has one.
    #[must_use]
    pub fn model_device(&self, source: &str, model: &str) -> Option<&str> {
        self.backends
            .models
            .get(source)
            .and_then(|m| m.get(model))
            .and_then(|s| s.device.as_deref())
    }

    /// The device `(source, model)` loads on: its own, else the global
    /// default. Every load path asks this, so a model with no device of its
    /// own keeps loading where the global preference always put it.
    #[must_use]
    pub fn effective_device(&self, source: &str, model: &str) -> String {
        self.model_device(source, model)
            .unwrap_or(&self.device.preferred_device)
            .to_string()
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
        // Selecting a model switches the stage on, exactly as
        // `enable_post_processor` does for stage 2.
        self.transcription.enabled = true;
        self.transcription.preferred_model = model;
        self.transcription.preferred_source = source;
        self.transcription.preferred_provider = provider.unwrap_or_default();
    }

    /// Stop running stage 1, keeping the selection so re-loading it is one
    /// call. The exact twin of [`Self::disable_post_processor`].
    ///
    /// This used to erase `preferred_model`/`preferred_source` outright — the
    /// only way, before the stage had an `enabled` flag, to keep a restart from
    /// reloading the model the user had just unloaded. The cost was that the
    /// card came back empty and the app had to remember the pick itself. The
    /// flag is what lets the selection stay.
    pub fn disable_transcription(&mut self) {
        self.transcription.enabled = false;
    }

    /// Set the active backend (its relative install dir) and drop the loaded
    /// model preference — selecting a backend does not load a model.
    pub fn update_active_backend(&mut self, dir: String) {
        self.transcription.active_backend = Some(dir);
        self.transcription.enabled = false;
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
        self.transcription.enabled = false;
        self.transcription.preferred_model = String::new();
        self.transcription.preferred_source = String::new();
        self.transcription.preferred_provider = String::new();
    }

    /// Select the post-processor model and whether it runs. Stored as the same
    /// `(name, source)` pair every model selection uses.
    pub fn enable_post_processor(&mut self, model: String, source: String) {
        self.post_processor.enabled = true;
        self.post_processor.model = model;
        self.post_processor.source = source;
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
mod device_config_tests {
    use super::*;

    /// A model with no device of its own loads where the global default
    /// says; one with its own loads there regardless of the default.
    #[test]
    fn a_model_device_overrides_the_global_default() {
        let mut cfg = DaemonConfig::default();
        assert_eq!(cfg.effective_device("s", "m"), "cpu");

        cfg.device.preferred_device = "gpu".to_string();
        assert_eq!(
            cfg.effective_device("s", "m"),
            "gpu",
            "inherits the default"
        );

        cfg.update_model_device("s", "m", Some("cpu".into()));
        assert_eq!(cfg.model_device("s", "m"), Some("cpu"));
        assert_eq!(cfg.effective_device("s", "m"), "cpu");
        assert_eq!(
            cfg.effective_device("s", "other"),
            "gpu",
            "a sibling model is untouched"
        );

        cfg.update_model_device("s", "m", None);
        assert_eq!(cfg.model_device("s", "m"), None);
        assert_eq!(cfg.effective_device("s", "m"), "gpu");
    }

    /// Setting a device on a model that already has a language keeps the
    /// language, and vice versa: both live in the same per-model row.
    #[test]
    fn device_and_language_share_the_model_row() {
        let mut cfg = DaemonConfig::default();
        cfg.update_model_language("s".into(), "m".into(), Some("fr".into()));
        cfg.update_model_device("s", "m", Some("gpu".into()));
        assert_eq!(cfg.model_language("s", "m"), Some("fr"));
        assert_eq!(cfg.model_device("s", "m"), Some("gpu"));

        let toml = toml::to_string(&cfg).expect("serialize");
        let back: DaemonConfig = toml::from_str(&toml).expect("deserialize");
        assert_eq!(back.model_language("s", "m"), Some("fr"));
        assert_eq!(back.model_device("s", "m"), Some("gpu"));
    }

    /// A persisted per-model device is normalized on load the same way the
    /// global default is: the deprecated `cuda` spelling still means `gpu`,
    /// and junk degrades to "no device of its own" rather than resetting the
    /// whole config.
    #[test]
    fn persisted_model_devices_are_normalized_on_load() {
        let content = r#"
[device]
preferred_device = "cpu"

[audio]
theme = "classic"
volume = 80

[transcription]
write_mode = true
recording_stop_mode = "manual_only"
write_method = "ydotool"

[backends.models."github.com/x/whisper".large]
device = "cuda"

[backends.models."github.com/x/whisper".small]
device = "xpu"
language = "fr"
"#;
        let (cfg, reset) = DaemonConfig::parse_or_reset(content);
        assert!(!reset);
        assert_eq!(
            cfg.model_device("github.com/x/whisper", "large"),
            Some("gpu")
        );
        assert_eq!(cfg.model_device("github.com/x/whisper", "small"), None);
        assert_eq!(
            cfg.model_language("github.com/x/whisper", "small"),
            Some("fr"),
            "normalizing the device leaves the rest of the row alone"
        );
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
