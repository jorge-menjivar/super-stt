// SPDX-License-Identifier: GPL-3.0-only
//! Removing the duplicate directories `dedup_sources` identified.
//!
//! Kept separate from discovery: reading a directory and deleting one are
//! different responsibilities, and destructive work does not belong inside a
//! function whose job is to scan.

use std::path::{Path, PathBuf};

use crate::stt_models::backends::DiscoveredBackend;
use super_stt_registry_types::manifest::Manifest;

/// What one reconciliation pass did: the bytes carried across, and the
/// directories it actually removed.
///
/// The removals are reported rather than merely counted because a caller has
/// to repair anything that pointed at one of them — see
/// [`repoint_active_backend`]. Basing that on "did the delete happen" rather
/// than on any one deleter's return value is what keeps the pointer and the
/// filesystem from disagreeing.
#[derive(Debug, Default)]
pub struct Reconciled {
    /// Bytes of model files moved from the losers into the winner.
    pub reclaimed: u64,
    /// The loser directories that no longer exist.
    pub removed: Vec<PathBuf>,
}

/// Move each loser's still-valid model files into `winner`, then remove it.
///
/// A directory whose manifest will not parse is left untouched: without a
/// readable file list there is no evidence it is the same backend, and that is
/// exactly the case where deleting would be a guess. A failed carry-over
/// aborts before the delete, so a partial move never costs the only copy.
pub async fn reconcile_dirs(losers: &[PathBuf], winner: &Path) -> Reconciled {
    let Ok(new) = Manifest::load(winner) else {
        log::error!(
            "Refusing to reconcile into {}: its manifest does not parse",
            winner.display()
        );
        return Reconciled::default();
    };

    let mut out = Reconciled::default();
    for loser in losers {
        let Ok(old) = Manifest::load(loser) else {
            log::error!(
                "Leaving {} in place: its manifest does not parse, so it cannot be \
                 confirmed a duplicate",
                loser.display()
            );
            continue;
        };
        let keep = crate::registry::carry_over::survivors(&old, &new);
        let bytes = match crate::registry::carry_over::carry(loser, winner, &keep).await {
            Ok(bytes) => {
                out.reclaimed += bytes;
                bytes
            }
            Err(e) => {
                log::error!(
                    "Leaving {} in place: carrying its model files into {} failed: {e}",
                    loser.display(),
                    winner.display()
                );
                continue;
            }
        };
        match tokio::fs::remove_dir_all(loser).await {
            Ok(()) => {
                log::info!(
                    "Reconciled duplicate {} ({}) into {} ({}), reclaiming {bytes} bytes",
                    loser.display(),
                    old.backend.version,
                    winner.display(),
                    new.backend.version,
                );
                out.removed.push(loser.clone());
            }
            Err(e) => log::error!("Failed to remove duplicate {}: {e}", loser.display()),
        }
    }
    out
}

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
        let done = reconcile_dirs(&dirs, &w.dir).await;
        reclaimed += done.reclaimed;
        repoint_active_backend(daemon, &done.removed, &w.dir).await;
    }
    reclaimed
}

/// Move `active_backend` — the persisted config and the runtime mirror — onto
/// `winner` when it named one of the directories just removed.
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
async fn repoint_active_backend(
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
        log::warn!("Failed to persist config after reconciling {active}: {e}");
    }
    log::info!("Repointed active_backend from {active} to {new_name} after reconciliation");
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn a_loser_hands_its_weights_over_before_it_is_removed() {
        let root = tempfile::tempdir().unwrap();
        let toml = r#"
[backend]
    source     = "github.com/x/y"
    name       = "Y"
    version    = "1.0.0"
    kind       = "subprocess"
    entrypoint = "y"
    contract   = "v1"
    license    = "Apache-2.0"
    description = "Test backend."

[[assets.subprocess]]
    file   = "y.tar.gz"
    target = "x86_64-unknown-linux-gnu"
    accel  = ["cpu"]

[[models]]
    name                = "m"
    primary_language    = "en"
    supported_languages = ["en"]
    supported_devices   = ["cpu"]
    files = [
        { url = "https://h/a.bin", destination = "models/m/a.bin" },
    ]
"#;
        let winner = root.path().join("app.super-stt.y");
        let loser = root.path().join("super-stt-y");
        for d in [&winner, &loser] {
            std::fs::create_dir_all(d.join("models/m")).unwrap();
            std::fs::write(d.join("backend.toml"), toml).unwrap();
        }
        std::fs::write(loser.join("models/m/a.bin"), b"weights").unwrap();

        let done = super::reconcile_dirs(&[loser.clone()], &winner).await;

        assert_eq!(done.reclaimed, 7);
        assert_eq!(done.removed, vec![loser.clone()]);
        assert!(
            winner.join("models/m/a.bin").exists(),
            "weights moved across"
        );
        assert!(!loser.exists(), "the duplicate is gone");
    }

    /// Beyond the byte count: the moved file's actual content must survive
    /// the carry, byte for byte. This is the case the task exists for — a
    /// count matching by coincidence would not catch a truncated or
    /// corrupted move.
    #[tokio::test]
    async fn a_losers_weights_genuinely_reach_the_winner_byte_for_byte() {
        let root = tempfile::tempdir().unwrap();
        let toml = r#"
[backend]
    source     = "github.com/x/y"
    name       = "Y"
    version    = "1.0.0"
    kind       = "subprocess"
    entrypoint = "y"
    contract   = "v1"
    license    = "Apache-2.0"
    description = "Test backend."

[[assets.subprocess]]
    file   = "y.tar.gz"
    target = "x86_64-unknown-linux-gnu"
    accel  = ["cpu"]

[[models]]
    name                = "m"
    primary_language    = "en"
    supported_languages = ["en"]
    supported_devices   = ["cpu"]
    files = [
        { url = "https://h/a.bin", destination = "models/m/a.bin" },
    ]
"#;
        let winner = root.path().join("app.super-stt.y");
        let loser = root.path().join("super-stt-y");
        for d in [&winner, &loser] {
            std::fs::create_dir_all(d.join("models/m")).unwrap();
            std::fs::write(d.join("backend.toml"), toml).unwrap();
        }
        // Realistic-shaped content, not just a short literal: several
        // repeated blocks so a truncation or a swapped chunk would be caught
        // by the equality check below.
        let content: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
        std::fs::write(loser.join("models/m/a.bin"), &content).unwrap();

        let done = super::reconcile_dirs(&[loser.clone()], &winner).await;

        assert_eq!(done.reclaimed, content.len() as u64);
        assert_eq!(done.removed, vec![loser.clone()]);
        let carried = std::fs::read(winner.join("models/m/a.bin")).unwrap();
        assert_eq!(
            carried, content,
            "the winner's copy must be byte-for-byte identical to the loser's"
        );
        assert!(!loser.exists(), "the duplicate is gone");
    }

    #[tokio::test]
    async fn an_unparseable_directory_is_left_alone() {
        let root = tempfile::tempdir().unwrap();
        let winner = root.path().join("app.super-stt.y");
        let loser = root.path().join("junk");
        std::fs::create_dir_all(&winner).unwrap();
        std::fs::create_dir_all(&loser).unwrap();
        std::fs::write(loser.join("backend.toml"), b"not toml {{{").unwrap();

        let done = super::reconcile_dirs(&[loser.clone()], &winner).await;

        assert_eq!(done.reclaimed, 0);
        assert!(
            done.removed.is_empty(),
            "nothing was removed, so nothing has to be repointed"
        );
        assert!(
            loser.exists(),
            "a directory we cannot read is never deleted"
        );
    }

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
