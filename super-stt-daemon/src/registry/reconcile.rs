// SPDX-License-Identifier: GPL-3.0-only
//! Removing the duplicate directories `dedup_sources` identified.
//!
//! Kept separate from discovery: reading a directory and deleting one are
//! different responsibilities, and destructive work does not belong inside a
//! function whose job is to scan.

use std::path::Path;

use crate::stt_models::backends::DiscoveredBackend;
use super_stt_registry_types::manifest::Manifest;

/// Move each loser's still-valid model files into `winner`, then remove it.
/// Returns the bytes carried across.
///
/// A directory whose manifest will not parse is left untouched: without a
/// readable file list there is no evidence it is the same backend, and that is
/// exactly the case where deleting would be a guess. A failed carry-over
/// aborts before the delete, so a partial move never costs the only copy.
pub async fn reconcile_dirs(losers: &[std::path::PathBuf], winner: &Path) -> u64 {
    let Ok(new) = Manifest::load(winner) else {
        log::error!(
            "Refusing to reconcile into {}: its manifest does not parse",
            winner.display()
        );
        return 0;
    };

    let mut reclaimed = 0u64;
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
                reclaimed += bytes;
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
            Ok(()) => log::info!(
                "Reconciled duplicate {} ({}) into {} ({}), reclaiming {bytes} bytes",
                loser.display(),
                old.backend.version,
                winner.display(),
                new.backend.version,
            ),
            Err(e) => log::error!("Failed to remove duplicate {}: {e}", loser.display()),
        }
    }
    reclaimed
}

/// Reconcile every duplicate `dedup_sources` reported, grouping each loser
/// with the winner that serves its `source`.
pub async fn reconcile(losers: &[DiscoveredBackend], winners: &[DiscoveredBackend]) -> u64 {
    let mut reclaimed = 0u64;
    for w in winners {
        let dirs: Vec<std::path::PathBuf> = losers
            .iter()
            .filter(|l| l.source == w.source)
            .map(|l| l.dir.clone())
            .collect();
        if !dirs.is_empty() {
            reclaimed += reconcile_dirs(&dirs, &w.dir).await;
        }
    }
    reclaimed
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

        let reclaimed = super::reconcile_dirs(&[loser.clone()], &winner).await;

        assert_eq!(reclaimed, 7);
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

        let reclaimed = super::reconcile_dirs(&[loser.clone()], &winner).await;

        assert_eq!(reclaimed, content.len() as u64);
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

        let reclaimed = super::reconcile_dirs(&[loser.clone()], &winner).await;

        assert_eq!(reclaimed, 0);
        assert!(
            loser.exists(),
            "a directory we cannot read is never deleted"
        );
    }
}
