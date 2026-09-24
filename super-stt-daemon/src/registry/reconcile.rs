// SPDX-License-Identifier: GPL-3.0-only
//! Removing the duplicate directories `dedup_sources` identified, and
//! repointing the active backend at the winner when it named one of them.
//!
//! The removal itself is `super_engine_daemon::registry::reconcile`, shared
//! with Super TTS; the active backend lives in Super STT's config, so
//! repointing it is here.

use std::path::{Path, PathBuf};

use crate::stt_models::backends::DiscoveredBackend;
pub use super_engine_daemon::registry::reconcile::{Reconciled, reconcile_dirs};

/// Reconcile every duplicate `dedup_sources` reported, grouping each loser
/// with the winner that serves its `source`, and move `active_backend` onto
/// the winner whenever the directory it named was one of the ones removed.
///
/// Returns the bytes carried across.
pub async fn reconcile(
    daemon: &crate::daemon::types::SuperSTTDaemon,
    losers: &[DiscoveredBackend],
    winners: &[DiscoveredBackend],
) -> u64 {
    let mut reclaimed = 0u64;
    for w in winners {
        let dirs: Vec<PathBuf> = losers
            .iter()
            .filter(|l| l.source == w.source)
            .map(|l| l.dir.clone())
            .collect();
        if dirs.is_empty() {
            continue;
        }
        let done = reconcile_dirs::<super_stt_registry_types::Stt>(&dirs, &w.dir).await;
        reclaimed += done.reclaimed;
        repoint_active_backend(daemon, &done.removed, &w.dir).await;
    }
    reclaimed
}

/// Move `active_backend` — the persisted config and the runtime mirror — onto
/// `winner` when it named one of the directories just removed: by
/// reconciliation here, or by an install that moved the backend to a new
/// directory name.
///
/// `active_backend` stores a directory name, and reconciliation is the last
/// thing standing between that name and a directory that no longer exists.
/// The pointer does not repair itself: `adopt_active_backend_for` only fills
/// an *unset* pointer, so a stale name simply resolves to nothing — the
/// daemon reports no active backend and serves no models, while the backend
/// itself is installed and healthy one directory over.
///
/// Keying this on the removal, rather than on any particular deleter
/// reporting success, is deliberate. Every route that removes a duplicate —
/// a first refresh cleaning up two pre-existing directories, an install whose
/// migration raced a concurrent refresh, a retirement whose `remove_dir_all`
/// failed and left the duplicate for a later pass — converges here, so there
/// is one place that has to be right instead of one per deleter.
///
/// `rename_active_backend` (not `update_active_backend`) is deliberate too:
/// the winner serves the same `source` and the same models, so this is a
/// directory move, not the user choosing a different backend. Clearing
/// `preferred_model`/`preferred_provider` here would silently discard the
/// user's model selection as a side effect of housekeeping.
pub(crate) async fn repoint_active_backend(
    daemon: &crate::daemon::types::SuperSTTDaemon,
    removed: &[PathBuf],
    winner: &Path,
) {
    let Some(new_name) = winner.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let mut cfg = daemon.config.write().await;
    let Some(active) = cfg.transcription.active_backend.clone() else {
        return;
    };
    // Losers and winner are siblings under the backends directory, so a name
    // match is an identity match: the pointer named a directory that is gone.
    let was_removed = removed
        .iter()
        .any(|d| d.file_name().and_then(|n| n.to_str()) == Some(active.as_str()));
    if !was_removed {
        return;
    }
    cfg.rename_active_backend(new_name.to_string());
    drop(cfg);
    *daemon.active_backend.write().await = Some(new_name.to_string());
    if let Err(e) = daemon.persist_config().await {
        log::warn!("Failed to persist config after repointing {active}: {e}");
    }
    log::info!("Repointed active_backend from {active} to {new_name}");
}

#[cfg(test)]
mod tests {
    /// A backend directory serving `github.com/x/y` at `version`, valid
    /// enough for `discover` to load it.
    fn write_backend(dir: &std::path::Path, version: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("backend.toml"),
            format!(
                r#"
[backend]
    source     = "github.com/x/y"
    name       = "Y"
    version    = "{version}"
    kind       = "subprocess"
    entrypoint = "y"
    contract   = "v1"
    description = "Test backend."

[[models]]
    name                = "m"
    primary_language    = "en"
    supported_languages = ["en"]
    supported_devices   = ["cpu"]
"#
            ),
        )
        .unwrap();
    }

    /// `active_backend` stores a directory name, and reconciliation is the
    /// only thing that removes a directory out from under it — including on
    /// the plain upgrade path, where a user who already has two directories
    /// for one source gets them reconciled on the first refresh. A pointer
    /// left naming the removed one resolves to nothing at all, and never
    /// self-heals: the adopt-on-startup path only fills an *unset* pointer.
    ///
    /// The model selection must survive: the winner serves the same source
    /// and the same models, so this is a directory move, not the user
    /// choosing a different backend.
    #[tokio::test]
    async fn reconciling_the_active_directory_repoints_it_and_keeps_the_model_choice() {
        let root = tempfile::tempdir().unwrap();
        let winner = root.path().join("app.super-stt.y");
        let loser = root.path().join("super-stt-y");
        write_backend(&winner, "1.0.1");
        write_backend(&loser, "1.0.0");

        let daemon = crate::daemon::types::test_daemon().await;
        {
            let mut cfg = daemon.config.write().await;
            cfg.transcription.active_backend = Some("super-stt-y".to_string());
            cfg.update_preferred_model(
                "m".to_string(),
                "github.com/x/y".to_string(),
                Some("local_y".to_string()),
            );
        }
        *daemon.active_backend.write().await = Some("super-stt-y".to_string());

        let (winners, losers) = crate::stt_models::backends::discover(root.path());
        assert_eq!(losers.len(), 1, "the older directory is the loser");
        super::reconcile(&daemon, &losers, &winners).await;

        assert!(!loser.exists(), "the duplicate is gone");
        let cfg = daemon.config.read().await;
        assert_eq!(
            cfg.transcription.active_backend.as_deref(),
            Some("app.super-stt.y"),
            "the pointer must follow the directory that survived"
        );
        assert_eq!(
            cfg.transcription.preferred_model, "m",
            "the model preference must survive the repoint"
        );
        assert_eq!(cfg.transcription.preferred_provider, "local_y");
        assert_eq!(cfg.transcription.preferred_source, "github.com/x/y");
        drop(cfg);
        assert_eq!(
            daemon.active_backend.read().await.as_deref(),
            Some("app.super-stt.y"),
            "the runtime mirror must agree with the persisted config"
        );
    }

    /// The mirror case: reconciling a backend the user has not selected must
    /// not move the pointer onto it.
    #[tokio::test]
    async fn reconciling_an_unrelated_directory_leaves_the_pointer_alone() {
        let root = tempfile::tempdir().unwrap();
        write_backend(&root.path().join("app.super-stt.y"), "1.0.1");
        write_backend(&root.path().join("super-stt-y"), "1.0.0");

        let daemon = crate::daemon::types::test_daemon().await;
        {
            let mut cfg = daemon.config.write().await;
            cfg.transcription.active_backend = Some("some-other-backend".to_string());
            cfg.update_preferred_model(
                "other-model".to_string(),
                "github.com/x/other".to_string(),
                Some("local_other".to_string()),
            );
        }

        let (winners, losers) = crate::stt_models::backends::discover(root.path());
        super::reconcile(&daemon, &losers, &winners).await;

        assert!(
            !root.path().join("super-stt-y").exists(),
            "the duplicate is still reconciled away"
        );
        let cfg = daemon.config.read().await;
        assert_eq!(
            cfg.transcription.active_backend.as_deref(),
            Some("some-other-backend"),
            "an unrelated active backend must not be repointed"
        );
        assert_eq!(cfg.transcription.preferred_model, "other-model");
    }
}
