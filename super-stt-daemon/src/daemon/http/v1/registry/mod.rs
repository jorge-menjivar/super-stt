// SPDX-License-Identifier: GPL-3.0-only
//! `/registry/backend/*` — the published catalog and the acts that change what
//! is installed from it.
//!
//! Contracts: `docs/protocol/endpoints/v1/registry/`.
//!
//! What each endpoint does is `super_engine_daemon::registry::endpoints`,
//! shared with Super TTS; the routes and their documentation are here, and so
//! is what the endpoints need from this daemon ([`RegistryHost`]). Removing a
//! backend is the one act that is not here — it belongs to the thing being
//! removed, at [`DELETE /backend/{backend_id}`](super::backends::uninstall_backend).
//!
//! These endpoints answer failures in their own envelope — see
//! [`registry_error`] — which is why they name `RegistryError` where the rest of
//! `/v1` names `ErrorEnvelope`.

pub(crate) mod install;
pub(crate) mod list;
pub(crate) mod preview;
pub(crate) mod refresh;
pub(crate) mod update;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::daemon::events::EventBus;
use crate::daemon::http::state::AppState;
use crate::daemon::types::SuperSTTDaemon;
use crate::stt_models::backends::DiscoveredBackend;
use super_engine_daemon::registry::Daemon;
use super_engine_daemon::registry::endpoints::RegistryHost;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// The registry error envelope. See
/// `super_engine_daemon::registry::endpoints::error`.
pub(crate) use super_engine_daemon::registry::endpoints::{
    error as registry_error, error_with_message as registry_error_msg,
};

/// Registry browse/refresh/install/update routes (settings-scope).
///
/// No path appears here: `routes!` reads each one off the handler's own
/// `#[utoipa::path]`, so the URL a client calls and the URL the document
/// publishes are the same string.
pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list::list_registry_backends))
        .routes(routes!(refresh::refresh_registry))
        .routes(routes!(install::install_registry_backend))
        .routes(routes!(preview::preview_registry_backend))
        .routes(routes!(update::update_registry_backend))
}

impl RegistryHost for SuperSTTDaemon {
    type Product = super_stt_registry_types::Stt;
    type Events = EventBus;

    fn daemon(&self) -> Daemon {
        crate::registry::DAEMON
    }

    fn events(&self) -> Arc<EventBus> {
        Arc::clone(&self.events)
    }

    fn backends(&self) -> &tokio::sync::RwLock<Vec<DiscoveredBackend>> {
        &self.backends
    }

    async fn backends_dir(&self) -> PathBuf {
        SuperSTTDaemon::backends_dir(self).await
    }

    /// Only the active backend is repointed. `rename_active_backend` keeps the
    /// model selection, since it is the same backend in a new directory.
    async fn backend_dirs_replaced(&self, removed: &[PathBuf], winner: &Path) {
        crate::registry::reconcile::repoint_active_backend(self, removed, winner).await;
    }

    async fn refresh_backends(&self) {
        SuperSTTDaemon::refresh_backends(self).await;
    }
}

#[cfg(test)]
mod tests {
    use crate::daemon::types::test_daemon;
    use super_engine_daemon::registry::endpoints::retire_and_repoint;

    /// Lay out an old directory serving `source` and a new one already
    /// installed at `new_dir_name`, as an install leaves them right before it
    /// retires the old one.
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
