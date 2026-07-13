// SPDX-License-Identifier: GPL-3.0-only
use crate::config::DaemonConfig;
use crate::daemon::events::EventBus;
use crate::download_progress::DownloadStateManager;
use crate::input::audio::AudioProcessor;
use crate::resource_management::ResourceManager;
use crate::services::dbus::DBusManager;
use crate::stt_models::backends::{self, DiscoveredBackend};
use anyhow::Result;
use std::sync::{Arc, RwLock};
use super_stt_shared::models::provider::Provider;
use super_stt_shared::theme::AudioTheme;
use tokio::sync::broadcast;

#[derive(Copy, Clone, Debug)]
pub enum DeviceOverride {
    Cpu,
    Cuda,
}

/// Normalize a backend-reported device label to the short wire-name
/// (`"cpu"` / `"cuda"` / `"metal"` / `"remote"`) used in
/// `daemon_status_changed` SSE payloads and on the `/active_device` endpoint.
#[must_use]
pub(crate) fn normalize_device(label: &str) -> String {
    let l = label.to_ascii_lowercase();
    if l.contains("cuda") {
        "cuda".to_string()
    } else if l.contains("metal") {
        "metal".to_string()
    } else if l.contains("remote") {
        "remote".to_string()
    } else {
        "cpu".to_string()
    }
}

/// A live model: its full resolved [`ModelDefinition`] plus the running
/// inference instance. Replaces what used to be parallel
/// `Arc<RwLock<Option<…>>>` slots for name, provider, and instance — those
/// always changed together and could drift during a switch. The definition
/// owns the name, provider, source, and architecture; nothing has to be
/// re-derived at read sites.
pub struct LoadedModel {
    pub definition: super_stt_shared::models::registry::ModelDefinition,
    pub instance: Box<dyn crate::stt_models::transcribe::Transcribe>,
}

/// Shared handle to the currently-loaded model (or `None` while idle/loading).
pub type SharedLoadedModel = Arc<tokio::sync::RwLock<Option<LoadedModel>>>;

/// The single `/transcribe` preview slot: an `(id, sender)` guarded by a lock,
/// where `id` lets a racing request claim the slot only when free and clear it
/// only when it is still its own.
pub type PreviewSlot =
    Arc<tokio::sync::RwLock<Option<(u64, tokio::sync::mpsc::UnboundedSender<String>)>>>;

#[derive(Clone)]
pub struct SuperSTTDaemon {
    pub model: SharedLoadedModel,
    pub audio_processor: Arc<AudioProcessor>,
    pub shutdown_tx: broadcast::Sender<()>,
    pub dbus_manager: Option<Arc<DBusManager>>,
    /// Internal pub/sub bus that fans recording / audio / STT events
    /// out to widget HTTP/SSE subscribers via `GET /events`.
    pub events: Arc<EventBus>,
    pub audio_theme: Arc<RwLock<AudioTheme>>,
    pub volume: Arc<RwLock<u8>>,
    pub busy: Arc<tokio::sync::RwLock<bool>>,
    pub download_manager: Arc<DownloadStateManager>,
    // Device management
    pub preferred_device: Arc<tokio::sync::RwLock<String>>, // "cpu" or "cuda"
    pub actual_device: Arc<tokio::sync::RwLock<String>>,    // actual device in use (may fallback)
    // Configuration management
    pub config: Arc<tokio::sync::RwLock<DaemonConfig>>,
    // Resource management for connection and rate limiting
    pub resource_manager: Arc<ResourceManager>,
    // Preview typing setting (beta feature)
    pub preview_typing_enabled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    // Sender used to signal a running recording to stop early (shortcut or external stop)
    pub manual_stop_tx: Arc<tokio::sync::RwLock<Option<tokio::sync::broadcast::Sender<()>>>>,
    // Cached keyboard simulator (session persists across recordings)
    pub simulator: Arc<tokio::sync::RwLock<Option<crate::output::keyboard::Simulator>>>,
    // Streams preview text to the one waiting `/transcribe` client. See
    // [`PreviewSlot`]: the id closes the busy-check TOCTOU that let a losing
    // request null the winner's preview stream.
    pub preview_text: PreviewSlot,
    // Backends discovered from the backends directory.
    pub backends: Arc<tokio::sync::RwLock<Vec<DiscoveredBackend>>>,
    // Active backend: the relative install dir (subdir of the backends dir) of
    // the selected provider, or None when idle. Runtime mirror of
    // `config.transcription.active_backend`.
    pub active_backend: Arc<tokio::sync::RwLock<Option<String>>>,
}

impl SuperSTTDaemon {
    /// Resolve a wire-level `(name, provider, source)` triple into a
    /// [`ModelDefinition`] from the discovered backends. Returns `None` on miss.
    pub async fn resolve_definition(
        &self,
        name: &str,
        provider: &Provider,
        source: &str,
    ) -> Option<super_stt_shared::models::registry::ModelDefinition> {
        let backends = self.backends.read().await;
        backends::find_model(&backends, name, provider, source).map(|(_, def)| def.clone())
    }

    /// Set the audio theme
    ///
    /// If the lock is poisoned, logs a warning and attempts to recover by creating a new lock.
    pub fn set_audio_theme(&self, theme: AudioTheme) {
        match self.audio_theme.write() {
            Ok(mut guard) => {
                *guard = theme;
                log::info!("Audio theme changed to: {theme}");
            }
            Err(poisoned) => {
                log::warn!("Audio theme lock was poisoned, attempting recovery");
                let mut guard = poisoned.into_inner();
                *guard = theme;
                log::info!("Audio theme changed to: {theme} (after lock recovery)");
            }
        }
    }

    /// Get the current audio theme
    ///
    /// If the lock is poisoned, logs a warning and returns the default theme.
    #[must_use]
    pub fn get_audio_theme(&self) -> AudioTheme {
        match self.audio_theme.read() {
            Ok(guard) => *guard,
            Err(poisoned) => {
                log::warn!("Audio theme lock was poisoned, returning current value");
                *poisoned.into_inner()
            }
        }
    }

    /// Set the master volume (0-100)
    pub fn set_volume(&self, volume: u8) {
        match self.volume.write() {
            Ok(mut guard) => {
                *guard = volume;
                log::info!("Volume changed to: {volume}");
            }
            Err(poisoned) => {
                log::warn!("Volume lock was poisoned, attempting recovery");
                let mut guard = poisoned.into_inner();
                *guard = volume;
                log::info!("Volume changed to: {volume} (after lock recovery)");
            }
        }
    }

    /// Get the current master volume (0-100)
    #[must_use]
    pub fn get_volume(&self) -> u8 {
        match self.volume.read() {
            Ok(guard) => *guard,
            Err(poisoned) => {
                log::warn!("Volume lock was poisoned, returning current value");
                *poisoned.into_inner()
            }
        }
    }

    /// Get the current volume as a f32 multiplier (0.0-1.0)
    #[must_use]
    pub fn get_volume_f32(&self) -> f32 {
        f32::from(self.get_volume()) / 100.0
    }

    /// Publish a recording-state transition on the SSE event bus so any
    /// connected `/events` subscribers (e.g. the COSMIC applet) update
    /// their visualization. The legacy notification path is gone; this
    /// is the only fan-out.
    pub fn broadcast_recording_state_change(&self, is_recording: bool) {
        self.events.publish_recording_state(is_recording);
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
        let config_guard = config.read().await;
        config_guard
            .save()
            .map_err(|e| anyhow::anyhow!("Failed to save config to disk: {e}"))?;
        log::debug!("Persisted config to disk");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_device_maps_labels() {
        assert_eq!(normalize_device("cuda:0"), "cuda");
        assert_eq!(normalize_device("Cuda(0)"), "cuda");
        assert_eq!(normalize_device("Metal(0)"), "metal");
        assert_eq!(normalize_device("remote"), "remote");
        assert_eq!(normalize_device("cpu"), "cpu");
        assert_eq!(normalize_device("anything else"), "cpu");
    }
}
