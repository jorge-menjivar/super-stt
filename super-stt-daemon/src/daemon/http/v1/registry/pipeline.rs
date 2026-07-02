// SPDX-License-Identifier: GPL-3.0-only
//! Shared background install/update pipeline machinery: the terminal-state
//! [`InflightGuard`] and the task spawner used by both the `install` and
//! `update` registry endpoints. Extracted from `install.rs` so `update.rs`
//! no longer reaches into the install handler for it.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock as ParkingRwLock;
use super_stt_shared::registry::events::RegistryEvent;

/// Ensures a spawned install/update task always reaches a terminal state. On
/// the happy path the task calls [`InflightGuard::disarm`] after emitting its
/// own `Completed`/`Failed` event. If the task instead unwinds (a panic in the
/// pipeline) before that, `Drop` clears the inflight marker — so retries are not
/// blocked by a stale `409` — and emits a `Failed` event so the client's
/// progress UI does not spin forever.
pub(super) struct InflightGuard {
    inflight: Arc<ParkingRwLock<HashSet<String>>>,
    events: Arc<crate::daemon::events::EventBus>,
    install_id: String,
    source: String,
    armed: bool,
}

impl InflightGuard {
    pub(super) fn new(
        inflight: Arc<ParkingRwLock<HashSet<String>>>,
        events: Arc<crate::daemon::events::EventBus>,
        install_id: String,
        source: String,
    ) -> Self {
        Self {
            inflight,
            events,
            install_id,
            source,
            armed: true,
        }
    }

    /// Normal completion: clear the inflight marker and suppress the
    /// unwind-path `Failed` event.
    pub(super) fn disarm(mut self) {
        self.inflight.write().remove(&self.source);
        self.armed = false;
    }
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        use super_stt_shared::registry::events::{InstallError, InstallPhase};
        if !self.armed {
            return;
        }
        self.inflight.write().remove(&self.source);
        let ev = RegistryEvent::Failed {
            install_id: self.install_id.clone(),
            source: self.source.clone(),
            phase: InstallPhase::Installing,
            error: InstallError::InstallIoError,
        };
        self.events
            .publish_registry_install(serde_json::to_value(ev).unwrap_or_default());
    }
}

/// Shared background pipeline spawner used by both the install and update
/// handlers. The caller has already inserted `source_bg` into the inflight
/// set; this function takes ownership of that responsibility via
/// [`InflightGuard`].
///
/// `local_src` is `Some` only for the install handler's local-path branch;
/// the update handler always passes `None`.
pub(super) fn spawn_install_pipeline(
    daemon: Arc<crate::daemon::types::SuperSTTDaemon>,
    inflight: Arc<ParkingRwLock<HashSet<String>>>,
    entry: crate::registry::index_schema::IndexBackend,
    sel: crate::registry::compat::Selection,
    install_id: String,
    source: String,
    local_src: Option<PathBuf>,
) {
    tokio::spawn(async move {
        // Always reach a terminal state, even if the pipeline panics.
        let guard = InflightGuard::new(
            inflight,
            Arc::clone(&daemon.events),
            install_id.clone(),
            source.clone(),
        );
        let backends_dir = {
            let c = daemon.config.read().await;
            c.transcription.backends_dir.clone().map_or_else(
                crate::stt_models::backends::default_backends_dir,
                PathBuf::from,
            )
        };
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("super-stt/install");

        let events = Arc::clone(&daemon.events);
        let install_id_ev = install_id.clone();
        let source_ev = source.clone();

        let pipeline = crate::registry::install::Pipeline {
            backends_dir,
            cache_dir,
            http: reqwest::Client::builder()
                // Bundles can be multi-GB (e.g. a CUDA backend's multi-part
                // archive), so allow a generous total timeout like the model
                // download client; the connect timeout still fails fast on an
                // unreachable host. A 5-minute total previously aborted large
                // GPU-bundle downloads as `DownloadFailed`.
                .timeout(Duration::from_hours(1))
                .connect_timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::limited(10))
                .build()
                .unwrap_or_default(),
            on_progress: Arc::new(move |phase, bytes: Option<(u64, Option<u64>)>| {
                use super_stt_shared::registry::events::{InstallPhase, RegistryEvent};
                let (bytes_done, bytes_total) = bytes.map_or((None, None), |(d, t)| (Some(d), t));
                let ev = RegistryEvent::Progress {
                    install_id: install_id_ev.clone(),
                    source: source_ev.clone(),
                    phase: match phase {
                        InstallPhase::Resolving => InstallPhase::Resolving,
                        InstallPhase::Downloading => InstallPhase::Downloading,
                        InstallPhase::Verifying => InstallPhase::Verifying,
                        InstallPhase::Extracting => InstallPhase::Extracting,
                        InstallPhase::Installing => InstallPhase::Installing,
                        InstallPhase::Rescanning => InstallPhase::Rescanning,
                    },
                    bytes_done,
                    bytes_total,
                };
                events.publish_registry_install(serde_json::to_value(ev).unwrap_or_default());
            }),
        };

        let events2 = Arc::clone(&daemon.events);
        let outcome = if let Some(src_dir) = local_src.as_ref() {
            crate::registry::install::run_local(&pipeline, &entry, src_dir).await
        } else {
            crate::registry::install::run(&pipeline, &entry, &sel).await
        };
        match outcome {
            Ok(version) => {
                // Refresh the daemon's in-memory backend catalog.
                daemon.refresh_backends().await;

                let ev = RegistryEvent::Completed {
                    install_id: install_id.clone(),
                    source: source.clone(),
                    version,
                };
                events2.publish_registry_install(serde_json::to_value(ev).unwrap_or_default());
            }
            Err((phase, error)) => {
                let ev = RegistryEvent::Failed {
                    install_id: install_id.clone(),
                    source: source.clone(),
                    phase,
                    error,
                };
                events2.publish_registry_install(serde_json::to_value(ev).unwrap_or_default());
            }
        }

        guard.disarm();
    });
}
