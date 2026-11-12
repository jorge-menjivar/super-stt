// SPDX-License-Identifier: GPL-3.0-only
use crate::config::DaemonConfig;
use crate::resource_management::ResourceManager;
use crate::tts::daemon::events::EventBus;
use crate::tts::download_progress::DownloadStateManager;
use crate::tts::tts_models::backends::{self, DiscoveredBackend};
use anyhow::Result;
use std::sync::Arc;
use super_engine_daemon::model_slot::{ModelSlot, SlotModel};
use super_engine_daemon::secrets::Secrets;
use super_voice_shared::tts::models::protocol::DaemonStatusEvent;
use tokio::sync::broadcast;

/// Normalize a backend-reported device label to the short wire-name
/// (`"cpu"` / `"cuda"` / `"rocm"` / `"metal"` / `"vulkan"` / `"remote"`) used
/// in `daemon_status_changed` SSE payloads and on the `/active_device`
/// endpoint.
#[must_use]
pub(crate) fn normalize_device(label: &str) -> String {
    let l = label.to_ascii_lowercase();
    if l.contains("cuda") {
        "cuda".to_string()
    } else if l.contains("rocm") || contains_hip_token(&l) {
        "rocm".to_string()
    } else if l.contains("vulkan") {
        "vulkan".to_string()
    } else if l.contains("metal") {
        "metal".to_string()
    } else if l.contains("remote") {
        "remote".to_string()
    } else {
        "cpu".to_string()
    }
}

/// Whether `label` (already lowercased) names `hip` as a whole token rather
/// than as a substring. A bare `contains("hip")` also fires inside `"chip"` /
/// `"chipset"`, so e.g. a Vulkan label mentioning an llvmpipe "chip" would
/// misroute to `rocm`.
fn contains_hip_token(label: &str) -> bool {
    label
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|token| token == "hip")
}

/// A live model: its full resolved [`ModelDefinition`] plus the running
/// inference instance. A single slot rather than parallel
/// `Arc<RwLock<Option<…>>>` slots for the name, the definition, and the
/// instance — those always changed together and could drift during a switch.
/// The definition owns the model's identity and capabilities, so nothing has
/// to be re-derived at read sites.
pub struct LoadedModel {
    pub definition: crate::tts::tts_models::ModelDefinition,
    pub instance: Box<dyn crate::tts::tts_models::synthesize::Synthesize>,
    /// Cloned voices already registered with `instance`, by wire id
    /// (`voice:<uuid>`).
    ///
    /// Registration is per loaded instance — a backend derives a speaker
    /// embedding from the reference clip and holds it in memory — so the set
    /// belongs to the same struct as the instance and dies with it. Here
    /// rather than inside each host so the two transports cannot disagree
    /// about when a voice counts as registered, and behind a `Mutex` because
    /// the speak path reaches it while holding only a read guard on the
    /// model slot.
    pub cloned_voices: parking_lot::Mutex<std::collections::HashSet<String>>,
}

impl LoadedModel {
    /// A freshly loaded model, with nothing registered yet.
    #[must_use]
    pub fn new(
        definition: crate::tts::tts_models::ModelDefinition,
        instance: Box<dyn crate::tts::tts_models::synthesize::Synthesize>,
    ) -> Self {
        Self {
            definition,
            instance,
            cloned_voices: parking_lot::Mutex::default(),
        }
    }
}

impl SlotModel for LoadedModel {
    async fn shutdown(&mut self) {
        if let Err(e) = self.instance.shutdown().await {
            log::warn!("backend shutdown failed: {e}");
        }
    }
}

/// Shared handle to the synthesis model's slot: the model (or `None` while
/// idle/loading), and every load and unload of it.
pub type SharedLoadedModel = Arc<ModelSlot<LoadedModel>>;

/// A load's turn at the synthesis slot: what
/// [`instantiate_backend`](SuperTTSDaemon::instantiate_backend) needs, and
/// what puts the model it loaded in the slot.
pub type LoadTurn<'a> = super_engine_daemon::model_slot::LoadTurn<'a, LoadedModel>;

/// Text to speech's part of the daemon: its model slot and speech engine, its
/// voices, its backends, and handles on what the daemon has once — the config,
/// the event bus, the secrets, the connection limits and the shutdown signal.
/// The handles are the daemon's own, shared with speech to text; see
/// [`Shared`].
#[derive(Clone)]
pub struct SuperTTSDaemon {
    pub model: SharedLoadedModel,
    pub shutdown_tx: broadcast::Sender<()>,
    /// The daemon's one event bus: playback and the synthesis stage's status
    /// fan out to `GET /events` subscribers through it.
    pub events: Arc<EventBus>,
    pub download_manager: Arc<DownloadStateManager>,

    // Configuration management
    pub config: Arc<tokio::sync::RwLock<DaemonConfig>>,
    // Resource management for connection and rate limiting
    pub resource_manager: Arc<ResourceManager>,
    // Backends discovered from the backends directory.
    pub backends: Arc<tokio::sync::RwLock<Vec<DiscoveredBackend>>>,
    // Active backend: the relative install dir (a subdir of the backends dir)
    // of the selected backend, or None when idle. Runtime mirror of
    // `config.synthesis.active_backend`.
    pub active_backend: Arc<tokio::sync::RwLock<Option<String>>>,
    // Desktop-notification channel for synthesis failures. Behind a mutex
    // because sending mutates the cached connection and the replaces-id.
    pub notifier: Arc<tokio::sync::Mutex<crate::tts::output::notification::Notifier>>,
    // The speak path: owns the output device (opened lazily, on first
    // utterance) and the in-flight utterance. See `crate::tts::daemon::speech`.
    pub speech: Arc<crate::tts::daemon::speech::SpeechEngine>,
    // Reference clips for cloned voices. Shared with `speech`, which resolves
    // a `voice:<uuid>` id against it. See `crate::tts::voices`.
    pub voices: Arc<crate::tts::voices::VoiceLibrary>,
    // The secrets file: the session tokens the daemon has issued and every
    // backend's API keys. One per process, opened at startup; clones share it.
    pub secrets: Secrets,
}

/// What text to speech is built with from the daemon: the handles both
/// features hold the same copy of.
pub struct Shared {
    pub config: Arc<tokio::sync::RwLock<DaemonConfig>>,
    pub events: Arc<EventBus>,
    pub secrets: Secrets,
    pub resource_manager: Arc<ResourceManager>,
    pub shutdown_tx: broadcast::Sender<()>,
}

/// Put `model` in `slot` on a load turn of its own, as a load would. For
/// tests that seed a loaded model.
#[cfg(test)]
pub(crate) async fn put_model(slot: &SharedLoadedModel, model: LoadedModel) {
    slot.begin_load()
        .await
        .expect("nothing else loads in a test")
        .install(model)
        .await;
}

/// A daemon wired up with inert defaults: no model, no backends, nothing
/// selected, and a `DaemonConfig::default()` that is never written to disk.
///
/// Lives beside the struct so there is exactly one copy — a second would drift
/// as fields are added (the compiler catches the drift, but only after someone
/// has fixed the same literal twice).
#[cfg(test)]
#[allow(clippy::unused_async)]
pub(crate) async fn test_daemon() -> SuperTTSDaemon {
    let (shutdown_tx, _) = broadcast::channel(1);
    SuperTTSDaemon {
        model: Arc::default(),
        shutdown_tx,
        events: Arc::new(EventBus::new()),
        download_manager: Arc::new(DownloadStateManager::new()),
        config: Arc::new(tokio::sync::RwLock::new(DaemonConfig::default())),
        resource_manager: Arc::new(ResourceManager::development()),
        backends: Arc::new(tokio::sync::RwLock::new(Vec::new())),
        active_backend: Arc::new(tokio::sync::RwLock::new(None)),
        // A fake notifier, not `Notifier::desktop()`: no daemon test may reach
        // the real session bus, or it can pop a genuine desktop notification
        // on whoever's machine runs the suite. `fail = true` mirrors a
        // headless/no-notification-server environment. A test that genuinely
        // needs delivery to succeed can override `daemon.notifier` after
        // construction.
        notifier: Arc::new(tokio::sync::Mutex::new(
            crate::tts::output::notification::Notifier::fake(true).0,
        )),
        // Detached: the test daemon must never claim a real output device.
        // A path unique to this daemon, never created unless something
        // writes to it: a shared one would let two tests see each other's
        // voices, and the real library belongs to the user's data dir.
        voices: Arc::new(crate::tts::voices::VoiceLibrary::new(
            std::env::temp_dir().join(format!("super-tts-test-voices-{}", uuid::Uuid::new_v4())),
        )),
        speech: Arc::new(crate::tts::daemon::speech::SpeechEngine::detached(
            crate::tts::audio::playback::DeviceFormat {
                sample_rate: 48000,
                channels: 1,
            },
        )),
        // In memory: no daemon test reads or writes the user's secrets file.
        secrets: Secrets::in_memory(),
    }
}

impl SuperTTSDaemon {
    /// Resolve a wire-level `(name, source)` pair into a [`ModelDefinition`]
    /// from the discovered backends. Returns `None` on miss.
    pub async fn resolve_definition(
        &self,
        name: &str,
        source: &str,
    ) -> Option<crate::tts::tts_models::ModelDefinition> {
        let backends = self.backends.read().await;
        backends::find_model(&backends, name, source).map(|(_, def)| def.clone())
    }

    /// Broadcast that a setting changed so subscribed clients re-resolve any
    /// derived state — e.g. a per-model language that follows the global
    /// value must re-fetch its resolution block. Reuses the
    /// `daemon_status_changed` topic clients already subscribe to;
    /// `setting` names what changed (currently `"language"`,
    /// `"update_check_enabled"`, or `"update_beta_optin"` — see
    /// `docs/protocol/endpoints/v1/events.md`). Shared by the language and
    /// settings handlers so the event shape can't drift between call sites.
    pub fn publish_settings_changed(&self, setting: &str) {
        self.events
            .publish_synthesis_status(DaemonStatusEvent::SettingsChanged {
                setting: setting.to_string(),
            });
    }

    /// Whether the model is currently occupied — an utterance is being
    /// synthesized or played out.
    ///
    /// Derived from the speech engine's slot rather than mirrored into a
    /// separate flag. The STT build carried a `busy: RwLock<bool>` alongside
    /// the recording task; two records of one fact drift, and the one that
    /// `/status` reports and uninstall waits on is the one you cannot afford
    /// to have stale.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.speech.current().is_some()
    }

    /// Persist the current config to disk. Settings handlers call this
    /// after mutating the in-memory config so the change survives a
    /// restart. The legacy `config_changed` broadcast that used to
    /// follow this save is no longer part of the documented protocol
    /// (see `docs/protocol/endpoints/v1/events.md`); a future
    /// cross-app sync mechanism should be added as a documented topic.
    ///
    /// # Errors
    ///
    /// Returns an error if the on-disk write fails.
    pub async fn persist_config(&self) -> Result<(), anyhow::Error> {
        Self::persist_config_static(&self.config).await
    }

    /// Static variant of [`persist_config`] for use in spawned tasks
    /// that hold a `Clone<Arc<RwLock<DaemonConfig>>>` directly.
    ///
    /// # Errors
    ///
    /// Returns an error if the on-disk write fails.
    pub async fn persist_config_static(
        config: &Arc<tokio::sync::RwLock<DaemonConfig>>,
    ) -> Result<(), anyhow::Error> {
        // Snapshot under the lock (cheap clone), release it, then do the
        // blocking TOML-serialize + `fs::write` on a blocking thread — never on
        // an async worker while a config lock is held (Tier 3 #3).
        let snapshot = config.read().await.clone();
        tokio::task::spawn_blocking(move || snapshot.save())
            .await
            .map_err(|e| anyhow::anyhow!("config-persist task failed: {e}"))?
            .map_err(|e| anyhow::anyhow!("Failed to save config to disk: {e}"))?;
        log::debug!("Persisted config to disk");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_device_maps_every_accel_label() {
        assert_eq!(normalize_device("cuda:0"), "cuda");
        assert_eq!(normalize_device("Cuda(0)"), "cuda");
        assert_eq!(normalize_device("Metal(0)"), "metal");
        assert_eq!(normalize_device("ROCm(0)"), "rocm");
        assert_eq!(normalize_device("hip:0"), "rocm");
        assert_eq!(normalize_device("Vulkan(0)"), "vulkan");
        assert_eq!(normalize_device("remote"), "remote");
        assert_eq!(normalize_device("cpu"), "cpu");
        assert_eq!(normalize_device("anything else"), "cpu");
    }

    /// `hip` must match as a token, not as a substring — a bare
    /// `contains("hip")` also fires inside `"chip"` and would misroute a
    /// Vulkan label naming an llvmpipe "chip" to `rocm`.
    #[test]
    fn normalize_device_does_not_treat_chip_as_hip() {
        assert_eq!(normalize_device("chip"), "cpu");
        assert_eq!(normalize_device("generic chip"), "cpu");
        assert_eq!(normalize_device("Vulkan (llvmpipe chip)"), "vulkan");
    }
}
