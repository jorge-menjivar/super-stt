// SPDX-License-Identifier: GPL-3.0-only
//! Shared background install/update pipeline machinery: the terminal-state
//! [`InflightGuard`] and the task spawner used by both the `install` and
//! `update` registry endpoints. Extracted from `install.rs` so `update.rs`
//! no longer reaches into the install handler for it.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

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

/// Removes a `source` from the inflight set when dropped — unless
/// [`InflightMarker::defuse`]d. Used by the *synchronous* pre-spawn phases of
/// the install/update handlers so every early error return cleans up the
/// inflight marker automatically instead of a hand-rolled
/// `inflight.write().remove(...)` at each bail site. Unlike [`InflightGuard`] it
/// emits no progress event: these phases fail with plain HTTP errors
/// (400/404/409/422/503), not `Failed` install events. On the happy path the
/// handler calls [`InflightMarker::defuse`] to hand the removal duty off to the
/// spawned pipeline's [`InflightGuard`].
pub(super) struct InflightMarker {
    inflight: Arc<ParkingRwLock<HashSet<String>>>,
    source: String,
    armed: bool,
}

impl InflightMarker {
    /// Insert `source` into the inflight set, returning a marker that removes it
    /// on drop. Returns `None` if `source` was already in flight (the set
    /// already contained it) — the caller maps that to its own `409`. The
    /// check-and-insert is atomic under one write lock.
    pub(super) fn acquire(
        inflight: Arc<ParkingRwLock<HashSet<String>>>,
        source: String,
    ) -> Option<Self> {
        if !inflight.write().insert(source.clone()) {
            return None;
        }
        Some(Self {
            inflight,
            source,
            armed: true,
        })
    }

    /// Hand the removal duty to the spawned pipeline's [`InflightGuard`]: leave
    /// the entry in place and do nothing on drop.
    pub(super) fn defuse(mut self) {
        self.armed = false;
    }
}

impl Drop for InflightMarker {
    fn drop(&mut self) {
        if self.armed {
            self.inflight.write().remove(&self.source);
        }
    }
}

/// After a successful install/update, retire the directory that used to
/// serve `source` (if the install just migrated to a new directory name) and
/// move `active_backend` — both the persisted config and the runtime mirror —
/// to follow it.
///
/// A no-op unless [`crate::registry::install::retire_previous_dir`] actually
/// removed a predecessor, and even then repoints `active_backend` only when
/// it named exactly the directory just retired: installing/updating a
/// backend that is not the one currently selected must never touch the
/// pointer. `rename_active_backend` (not `update_active_backend`) is
/// deliberate — this is the same backend, same models, only the directory
/// name changed, so the user's model selection must survive.
async fn retire_and_repoint(
    daemon: &crate::daemon::types::SuperSTTDaemon,
    backends_dir: &std::path::Path,
    source: &str,
    dir_name: &str,
) {
    let installed_at = backends_dir.join(dir_name);
    let Some(old) =
        crate::registry::install::retire_previous_dir(backends_dir, source, &installed_at).await
    else {
        return;
    };
    let old_name = old
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();

    let mut cfg = daemon.config.write().await;
    if cfg.transcription.active_backend.as_deref() != Some(old_name.as_str()) {
        return;
    }
    cfg.rename_active_backend(dir_name.to_string());
    drop(cfg);
    *daemon.active_backend.write().await = Some(dir_name.to_string());
    if let Err(e) = daemon.persist_config().await {
        log::warn!("Failed to persist config after backend migration rename: {e}");
    }
    log::info!("Repointed active_backend from {old_name} to {dir_name}");
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
        let cache_dir = super_stt_shared::paths::cache_dir().join("install");

        let events = Arc::clone(&daemon.events);
        let install_id_ev = install_id.clone();
        let source_ev = source.clone();

        let pipeline = crate::registry::install::Pipeline {
            backends_dir,
            cache_dir,
            // Bundles can be multi-GB (e.g. a CUDA backend's multi-part
            // archive), so use the generous-timeout download client; the
            // connect timeout still fails fast on an unreachable host.
            http: super_stt_forge::http::download_client(),
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
                // An update that installs a version declaring an `id` may
                // land at a new directory name while the backend is still
                // installed under its old one — retire the predecessor and
                // move the active-backend pointer with it before the catalog
                // is rescanned.
                let dir_name = crate::registry::install_dir_name(&entry);
                retire_and_repoint(&daemon, &pipeline.backends_dir, &entry.source, dir_name).await;

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

#[cfg(test)]
mod tests {
    use super::retire_and_repoint;
    use crate::daemon::types::test_daemon;

    /// Lay out an old directory serving `source` and a new one already
    /// installed at `new_dir_name`, matching what `run`/`run_local` leave on
    /// disk right before this function runs in production.
    fn migrated_layout(root: &std::path::Path, old_dir_name: &str, new_dir_name: &str) {
        let old = root.join(old_dir_name);
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(
            old.join("backend.toml"),
            r#"
[backend]
source = "github.com/x/voxtral"
name = "Voxtral"
version = "1.0.0"
kind = "subprocess"
entrypoint = "voxtral"
contract = "v1"
description = "Test backend."
"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join(new_dir_name)).unwrap();
    }

    #[tokio::test]
    async fn a_migration_repoints_active_backend_when_it_named_the_retired_directory() {
        let root = tempfile::tempdir().unwrap();
        migrated_layout(root.path(), "super-stt-voxtral", "app.super-stt.voxtral");

        let daemon = test_daemon().await;
        daemon.config.write().await.transcription.active_backend =
            Some("super-stt-voxtral".to_string());
        daemon.config.write().await.update_preferred_model(
            "voxtral-mini".to_string(),
            "github.com/x/voxtral".to_string(),
            Some("local_voxtral".to_string()),
        );

        retire_and_repoint(
            &daemon,
            root.path(),
            "github.com/x/voxtral",
            "app.super-stt.voxtral",
        )
        .await;

        assert!(!root.path().join("super-stt-voxtral").exists());
        let cfg = daemon.config.read().await;
        assert_eq!(
            cfg.transcription.active_backend.as_deref(),
            Some("app.super-stt.voxtral"),
            "the pointer must follow the migration"
        );
        assert_eq!(
            cfg.transcription.preferred_model, "voxtral-mini",
            "the model preference must survive the rename"
        );
        assert_eq!(cfg.transcription.preferred_provider, "local_voxtral");
        drop(cfg);
        assert_eq!(
            daemon.active_backend.read().await.as_deref(),
            Some("app.super-stt.voxtral"),
            "the runtime mirror must agree with the persisted config"
        );
    }

    #[tokio::test]
    async fn a_migration_leaves_a_different_active_backend_untouched() {
        let root = tempfile::tempdir().unwrap();
        migrated_layout(root.path(), "super-stt-voxtral", "app.super-stt.voxtral");

        let daemon = test_daemon().await;
        daemon.config.write().await.transcription.active_backend =
            Some("some-other-backend".to_string());
        daemon.config.write().await.update_preferred_model(
            "other-model".to_string(),
            "github.com/x/other".to_string(),
            Some("local_other".to_string()),
        );

        retire_and_repoint(
            &daemon,
            root.path(),
            "github.com/x/voxtral",
            "app.super-stt.voxtral",
        )
        .await;

        // The stale directory is still retired — that part is unconditional —
        // but the pointer, which names a different backend entirely, must not
        // move.
        assert!(
            !root.path().join("super-stt-voxtral").exists(),
            "the predecessor is retired regardless of what's active"
        );
        let cfg = daemon.config.read().await;
        assert_eq!(
            cfg.transcription.active_backend.as_deref(),
            Some("some-other-backend"),
            "an unrelated active backend must not be repointed"
        );
        assert_eq!(cfg.transcription.preferred_model, "other-model");
        assert_eq!(cfg.transcription.preferred_provider, "local_other");
        drop(cfg);
        assert_eq!(daemon.active_backend.read().await.as_deref(), None);
    }
}
