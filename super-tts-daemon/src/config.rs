// SPDX-License-Identifier: GPL-3.0-only
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use super_tts_shared::models::notification_method::NotificationMethod;
use super_tts_shared::models::update_beta_optin::UpdateBetaOptIn;
use super_tts_shared::theme::AudioTheme;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    pub device: DeviceConfig,
    pub audio: AudioConfig,
    pub synthesis: SynthesisConfig,
    #[serde(default)]
    pub backends: BackendsConfig,
    #[serde(default)]
    pub update: UpdateConfig,
    #[serde(default)]
    pub http: HttpConfig,
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
        deserialize_with = "super_tts_shared::utils::serde_helpers::deserialize_or_default"
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

/// User-set configuration for installed backends. Secrets live in the
/// secrets file; only non-sensitive **options** are stored here (plaintext).
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
    /// because it is a property of the model: a small voice runs fine on the
    /// CPU while the large one beside it needs the GPU, and one global switch
    /// forced a user who wanted the GPU for one of them to accept it for both.
    #[serde(default)]
    pub device: Option<String>,
    /// The voice this model speaks in when an utterance names none: a `voice`
    /// id in any of the three shapes `docs/protocol/backend/config.md` defines,
    /// or `None` for the model's manifest `default_voice`.
    ///
    /// Per model rather than global for the same reason the device is: a voice
    /// id means nothing to another model. `ryan` is one of the nine speakers
    /// the `CustomVoice` checkpoints declare and is not a voice any Kokoro build
    /// has, and a cloned `voice:<uuid>` is refused outright by a model whose
    /// `voice_kinds` do not include `cloned`. One global voice would be wrong
    /// for every model but the one it was picked for.
    #[serde(default)]
    pub voice: Option<String>,
}

/// The device models fall back to when they have none of their own
/// ([`ModelSettings::device`]).
///
/// Still settable through `set_device`, which is the *default* rather than any
/// one model's choice, and still what a config written before per-model
/// devices existed carries — so a user upgrading keeps loading models exactly
/// where they always loaded, without any rewrite of their config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub preferred_device: String, // "cpu" or "gpu"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    #[serde(
        default,
        deserialize_with = "super_tts_shared::utils::serde_helpers::deserialize_or_default"
    )]
    pub theme: AudioTheme,
    #[serde(default = "default_volume")]
    pub volume: u8,
}

fn default_volume() -> u8 {
    100
}

/// The daemon's HTTP surface beyond the Unix socket it always serves. See
/// `super_engine_daemon::config::HttpConfig`, bound to [`DEFAULT_TCP_PORT`].
pub type HttpConfig = super_engine_daemon::config::HttpConfig<DEFAULT_TCP_PORT>;

/// The loopback TCP listener. See `super_engine_daemon::config::TcpConfig`.
pub type TcpConfig = super_engine_daemon::config::TcpConfig<DEFAULT_TCP_PORT>;

pub use super_engine_daemon::config::ANY_ORIGIN;

/// The loopback port the daemon serves on unless told otherwise.
///
/// Public because it is not only a default: it is the number a browser client
/// hard-codes, the one the published `OpenAPI` document advertises as a server,
/// and the one the protocol docs quote. Those must all move together, so they
/// all read it from here.
///
/// **7300–7309 is the block reserved for the Super family**, one port per
/// daemon: `7300` Super STT, `7301` Super TTS, the rest unused for now. A block
/// rather than a number apiece so a sibling never has to repeat this search,
/// and so a firewall rule or a document can name the whole family at once.
///
/// The band was picked for how empty it is rather than for looking nice. The
/// 7000s has no conventional block reservation — unlike the 5000s, where VNC
/// takes 5900–5999, and the 6000s, where X11 takes 6000–6063 — and 7284–7364
/// is 81 consecutive ports unassigned in IANA's registry, `genstat` at 7283
/// below and `lcm-server` at 7365 above. This block sits in the middle of that.
/// The busy parts of the 7000s are all well outside it: 7860 is Gradio's
/// default, which matters more than its absence from the registry suggests
/// since it is where ML demo apps land; 7777, 7474 and 7687 are likewise taken
/// in practice.
///
/// See [`TcpConfig::port`] for why a fixed number rather than an OS-chosen one.
/// It is also the product's `ProductSpec::tcp_port`, which a test holds it to.
pub const DEFAULT_TCP_PORT: u16 = 7301;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthesisConfig {
    #[serde(default)]
    pub preferred_model: String,
    #[serde(default)]
    pub preferred_source: String,
    /// How a synthesis failure is surfaced to the user. An unparseable stored
    /// value degrades to the default rather than failing the whole config load.
    #[serde(
        default,
        deserialize_with = "super_tts_shared::utils::serde_helpers::deserialize_or_default"
    )]
    pub notification_method: NotificationMethod,
    /// Directory scanned for installed backends. `None` uses the default
    /// (`<data_dir>/super-tts/backends`).
    #[serde(default)]
    pub backends_dir: Option<String>,
    /// Relative install dir (subdir of [`backends_dir`]) of the selected active
    /// backend, or `None` when idle. Metadata (name/source/models) is read from
    /// that dir's `backend.toml`. An active backend with an empty
    /// `preferred_model` means "backend selected, no model loaded".
    #[serde(default)]
    pub active_backend: Option<String>,
    /// Global default synthesis language: a BCP-47 tag, the reserved
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
            synthesis: SynthesisConfig {
                // Empty preference: the daemon stays idle until a model is
                // selected — it never auto-picks one, since loading a model can
                // pull gigabytes.
                preferred_model: String::new(),
                preferred_source: String::new(),
                notification_method: NotificationMethod::default(),
                backends_dir: None,
                active_backend: None,
                primary_language: None,
            },
            backends: BackendsConfig::default(),
            update: UpdateConfig::default(),
            http: HttpConfig::default(),
        }
    }
}

/// The `daemon.ron` path used by `DaemonConfig::get_config_path()` under
/// `#[cfg(test)]`: a directory under the OS temp dir, unique per test
/// *process* (keyed by pid) and cached for the life of that process so every
/// test in the binary agrees on the same path. This is what keeps unit tests
/// that call `load()`/`save()` off the developer's real config file.
#[cfg(test)]
fn test_config_path() -> PathBuf {
    use std::sync::OnceLock;
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    // A static is never dropped, so the directory is removed when the test
    // binary exits instead, failed tests or not.
    extern "C" fn remove_test_config_dir() {
        if let Some(dir) = PATH.get().and_then(|path| path.parent()) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
    PATH.get_or_init(|| {
        let dir =
            std::env::temp_dir().join(format!("super-tts-test-config-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // SAFETY: registers a plain `extern "C" fn` with no captured state.
        unsafe {
            libc::atexit(remove_test_config_dir);
        }
        dir.join("daemon.ron")
    })
    .clone()
}

/// Devices are bare `String`s, not an enum, so no `deserialize_or_default`
/// catches a stale value at the serde layer — they are normalized here
/// instead. Unlike the wire setter, which rejects an unparseable value
/// outright, a persisted value has no such option: a daemon that refused to
/// start over a stale config field would be worse than one that falls back to
/// the default, so the global default degrades to `"cpu"` and a per-model
/// device to "none of its own" rather than triggering a full reset. The
/// deprecated `cuda`/`metal` spellings normalize to `gpu` rather than falling
/// back, the same as the wire setter, so a config written before this
/// vocabulary still loads onto the accelerator it always meant.
impl super_engine_daemon::config::ConfigFile for DaemonConfig {
    fn normalize(&mut self) {
        use crate::daemon::device_management::parse_device_preference;

        self.device.preferred_device = parse_device_preference(&self.device.preferred_device)
            .unwrap_or_else(|| "cpu".to_string());
        for settings in self
            .backends
            .models
            .values_mut()
            .flat_map(HashMap::values_mut)
        {
            settings.device = settings.device.as_deref().and_then(parse_device_preference);
        }
    }
}

impl DaemonConfig {
    /// Get the config file path.
    ///
    /// Under `#[cfg(test)]` this resolves to a process-local temp file instead
    /// of the real XDG config path, so `load()`/`save()` in this crate's unit
    /// tests can never read or clobber the developer's real
    /// `~/.config/super-tts/daemon.ron`. Known limitation: this only covers
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
            super_engine_daemon::config::config_path(&super_tts_shared::product::SUPER_TTS)
        }
    }

    /// Parse config file `content`, falling back to defaults on a parse
    /// error. See `super_engine_daemon::config::parse_or_reset`; what a
    /// stale value is repaired to is `ConfigFile::normalize` above.
    #[cfg(test)]
    fn parse_or_reset(content: &str) -> (Self, bool) {
        super_engine_daemon::config::parse_or_reset(content)
    }

    /// Load configuration from disk. See `super_engine_daemon::config::load`.
    #[must_use]
    pub fn load() -> Self {
        super_engine_daemon::config::load(&Self::get_config_path())
    }

    /// Save configuration to disk. Blocking (`std::fs::write`); on the async
    /// runtime call it via `persist_config()` / `spawn_blocking`, not inline.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration directory cannot be created,
    /// serialization fails, or the file cannot be written.
    pub fn save(&self) -> anyhow::Result<()> {
        super_engine_daemon::config::save(&Self::get_config_path(), self)
    }

    // Config mutators are PURE (Tier 3 #3): they mutate in-memory state only.
    // The caller persists once via `persist_config()` (which writes off the
    // async runtime), so there are no blocking `fs::write`s under the config
    // lock and no double writes.

    /// Update the global default device — the one models with no device of
    /// their own fall back to. Per-model choices are untouched, which is the
    /// point of the split: flipping the default must not silently move a model
    /// the user deliberately pinned somewhere else.
    pub fn update_preferred_device(&mut self, device: String) {
        self.device.preferred_device = device;
    }

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
    /// default. Every load path asks this rather than reading
    /// `device.preferred_device` directly, so a model with no device of its
    /// own keeps loading where the global preference always put it, and one
    /// with its own is not dragged along by a change to that default.
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

    /// Update preferred model + source.
    pub fn update_preferred_model(&mut self, model: String, source: String) {
        self.synthesis.preferred_model = model;
        self.synthesis.preferred_source = source;
    }

    /// Clear the loaded-model preference (model + source) while keeping the
    /// active backend selected. Used by the unload path so a daemon restart
    /// stays idle instead of reloading the unloaded model.
    pub fn clear_preferred_model(&mut self) {
        self.synthesis.preferred_model = String::new();
        self.synthesis.preferred_source = String::new();
    }

    /// Set the active backend (its relative install dir) and drop the loaded
    /// model preference — selecting a backend does not load a model.
    pub fn update_active_backend(&mut self, dir: String) {
        self.synthesis.active_backend = Some(dir);
        self.synthesis.preferred_model = String::new();
    }

    /// Repoint the active backend to `new_dir` without touching the loaded
    /// model preference.
    ///
    /// Deliberately narrower than [`update_active_backend`]: that method
    /// clears `preferred_model` because it models the
    /// user *choosing a different backend*, where any previously loaded
    /// model no longer applies. This method models an install-time directory
    /// rename of the *same* backend — its identity, models, and selection are
    /// unchanged, only the on-disk directory name moved. Using
    /// `update_active_backend` here would silently wipe the user's model
    /// choice as a side effect of an update. Do not merge the two.
    pub fn rename_active_backend(&mut self, new_dir: String) {
        self.synthesis.active_backend = Some(new_dir);
    }

    /// Clear the active backend and the loaded-model preference (→ idle).
    pub fn clear_active_backend(&mut self) {
        self.synthesis.active_backend = None;
        self.synthesis.preferred_model = String::new();
        self.synthesis.preferred_source = String::new();
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
        self.synthesis.primary_language = language;
    }

    #[must_use]
    pub fn primary_language(&self) -> Option<&str> {
        self.synthesis.primary_language.as_deref()
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

    /// Set (`Some`) or clear (`None`) the voice a model speaks in by default.
    pub fn update_model_voice(&mut self, source: String, model: String, voice: Option<String>) {
        match voice {
            Some(v) => {
                self.backends
                    .models
                    .entry(source)
                    .or_default()
                    .entry(model)
                    .or_default()
                    .voice = Some(v);
            }
            None => {
                if let Some(models) = self.backends.models.get_mut(&source)
                    && let Some(settings) = models.get_mut(&model)
                {
                    settings.voice = None;
                }
            }
        }
    }

    #[must_use]
    pub fn model_voice(&self, source: &str, model: &str) -> Option<&str> {
        self.backends
            .models
            .get(source)
            .and_then(|m| m.get(model))
            .and_then(|s| s.voice.as_deref())
    }
}

#[cfg(test)]
mod language_config_tests {
    use super::*;

    #[test]
    fn primary_and_model_language_round_trip_through_the_file() {
        let mut cfg = DaemonConfig::default();
        assert_eq!(cfg.primary_language(), None);

        cfg.update_primary_language(Some("es-MX".to_string()));
        cfg.update_model_language(
            "github.com/x/kokoro".to_string(),
            "kokoro-large".to_string(),
            Some("fr".to_string()),
        );

        let written = super_engine_daemon::config::to_string(&cfg).expect("serialize");
        let (back, reset) = DaemonConfig::parse_or_reset(&written);
        assert!(!reset);

        assert_eq!(back.primary_language(), Some("es-MX"));
        assert_eq!(
            back.model_language("github.com/x/kokoro", "kokoro-large"),
            Some("fr")
        );
        assert_eq!(back.model_language("github.com/x/kokoro", "absent"), None);
    }

    /// The voice rides in the same per-model record as the language and the
    /// device, so it has to survive the same round trip through the file.
    #[test]
    fn a_model_voice_round_trips_through_the_file() {
        let mut cfg = DaemonConfig::default();
        cfg.update_model_voice(
            "github.com/x/qwen".into(),
            "qwen3-tts-0.6b-custom-voice".into(),
            Some("aiden".into()),
        );
        let written = super_engine_daemon::config::to_string(&cfg).expect("serialize");
        let (back, reset) = DaemonConfig::parse_or_reset(&written);
        assert!(!reset);
        assert_eq!(
            back.model_voice("github.com/x/qwen", "qwen3-tts-0.6b-custom-voice"),
            Some("aiden")
        );
        assert_eq!(back.model_voice("github.com/x/qwen", "absent"), None);
    }

    /// A voice and a language set on the same model do not overwrite each
    /// other: both live in one `ModelSettings`, so a careless `or_default`
    /// would blank whichever was written first.
    #[test]
    fn a_voice_and_a_language_coexist_on_one_model() {
        let mut cfg = DaemonConfig::default();
        cfg.update_model_language("s".into(), "m".into(), Some("fr".into()));
        cfg.update_model_voice("s".into(), "m".into(), Some("ryan".into()));
        assert_eq!(cfg.model_language("s", "m"), Some("fr"));
        assert_eq!(cfg.model_voice("s", "m"), Some("ryan"));

        // And clearing one leaves the other alone.
        cfg.update_model_voice("s".into(), "m".into(), None);
        assert_eq!(cfg.model_voice("s", "m"), None);
        assert_eq!(cfg.model_language("s", "m"), Some("fr"));
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

        let written = super_engine_daemon::config::to_string(&cfg).expect("serialize");
        let (back, reset) = DaemonConfig::parse_or_reset(&written);
        assert!(!reset);
        assert_eq!(back.model_language("s", "m"), Some("fr"));
        assert_eq!(back.model_device("s", "m"), Some("gpu"));
    }

    /// A persisted per-model device is normalized on load the same way the
    /// global default is: the deprecated `cuda` spelling still means `gpu`,
    /// and junk degrades to "no device of its own" rather than resetting the
    /// whole config.
    #[test]
    fn persisted_model_devices_are_normalized_on_load() {
        let content = r#"(
    device: (preferred_device: "cpu"),
    audio: (theme: "classic", volume: 80),
    synthesis: (preferred_model: ""),
    backends: (
        models: {
            "github.com/x/kokoro": {
                "large": (device: Some("cuda")),
                "small": (device: Some("xpu"), language: Some("fr")),
            },
        },
    ),
)"#;
        let (cfg, reset) = DaemonConfig::parse_or_reset(content);
        assert!(!reset);
        assert_eq!(
            cfg.model_device("github.com/x/kokoro", "large"),
            Some("gpu")
        );
        assert_eq!(cfg.model_device("github.com/x/kokoro", "small"), None);
        assert_eq!(
            cfg.model_language("github.com/x/kokoro", "small"),
            Some("fr"),
            "normalizing the device leaves the rest of the row alone"
        );
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
