// SPDX-License-Identifier: GPL-3.0-only
//! Which downloaded model files survive a backend directory being replaced.
//!
//! Replacing a backend directory used to take its `models/` subtree with it,
//! so an update discarded gigabytes of weights that immediately re-downloaded.
//! A file is still valid when both manifests declare the same `destination`
//! with the same `url`: a destination the new manifest no longer declares is a
//! deleted file, and a changed `url` at the same destination is a changed one.
//!
//! `sha256` is deliberately not part of the predicate. `download::usable_existing`
//! re-verifies every carried file against the new manifest's hash at provision
//! time and re-downloads on mismatch, so a stale hash cannot survive; checking
//! it here would only duplicate that.

use std::collections::HashMap;
use std::path::Path;

use super_stt_registry_types::manifest::Manifest;
use tokio::fs;

/// Map every declared file destination to its download URL.
fn declared(m: &Manifest) -> HashMap<&str, &str> {
    m.models
        .iter()
        .flat_map(|model| &model.files)
        .map(|f| (f.destination.as_str(), f.url.as_str()))
        .collect()
}

/// Destinations declared by both manifests with the same URL, sorted so the
/// result is stable for logging and tests.
///
/// Every destination is validated as a safe relative path by `Manifest::parse`,
/// so callers may join these onto a directory without escaping it.
#[must_use]
pub fn survivors(old: &Manifest, new: &Manifest) -> Vec<String> {
    let old_files = declared(old);
    let mut out: Vec<String> = declared(new)
        .into_iter()
        .filter(|(dest, url)| old_files.get(dest) == Some(url))
        .map(|(dest, _)| dest.to_string())
        .collect();
    out.sort();
    out
}

/// Move each of `destinations` from `from_dir` into `to_dir`, returning the
/// total bytes moved.
///
/// A destination missing under `from_dir` is skipped, and one that already
/// exists under `to_dir` is left alone — the staged copy is the newer one.
/// Both directories live under the backends directory, so these are
/// same-filesystem renames: constant-time, with no multi-gigabyte copy.
///
/// # Errors
/// Returns an `io::Error` if a directory cannot be created or a rename fails.
pub async fn carry(
    from_dir: &Path,
    to_dir: &Path,
    destinations: &[String],
) -> std::io::Result<u64> {
    let mut moved = 0u64;
    for dest in destinations {
        let src = from_dir.join(dest);
        let dst = to_dir.join(dest);
        let Ok(meta) = fs::metadata(&src).await else {
            continue;
        };
        if fs::metadata(&dst).await.is_ok() {
            continue;
        }
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::rename(&src, &dst).await?;
        moved += meta.len();
    }
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::{carry, survivors};
    use super_stt_registry_types::manifest::Manifest;

    fn manifest_with(files: &[(&str, &str)]) -> Manifest {
        let entries = files
            .iter()
            .map(|(url, dest)| format!("{{ url = \"{url}\", destination = \"{dest}\" }}"))
            .collect::<Vec<_>>()
            .join(",\n            ");
        let text = format!(
            r#"
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
    name               = "m"
    primary_language   = "en"
    supported_languages = ["en"]
    supported_devices  = ["cpu"]
    files = [
            {entries}
    ]
"#
        );
        Manifest::parse(&text).expect("fixture manifest parses")
    }

    #[test]
    fn an_unchanged_url_at_the_same_destination_survives() {
        let old = manifest_with(&[("https://h/a.bin", "models/m/a.bin")]);
        let new = manifest_with(&[("https://h/a.bin", "models/m/a.bin")]);
        assert_eq!(survivors(&old, &new), vec!["models/m/a.bin".to_string()]);
    }

    #[test]
    fn a_changed_url_does_not_survive() {
        let old = manifest_with(&[("https://h/a.bin", "models/m/a.bin")]);
        let new = manifest_with(&[("https://h/a-v2.bin", "models/m/a.bin")]);
        assert!(survivors(&old, &new).is_empty());
    }

    #[test]
    fn a_destination_the_new_manifest_drops_does_not_survive() {
        let old = manifest_with(&[("https://h/a.bin", "models/m/a.bin")]);
        let new = manifest_with(&[("https://h/b.bin", "models/m/b.bin")]);
        assert!(survivors(&old, &new).is_empty());
    }

    #[tokio::test]
    async fn carry_moves_only_what_is_present_and_absent_at_the_destination() {
        let from = tempfile::tempdir().unwrap();
        let to = tempfile::tempdir().unwrap();

        std::fs::create_dir_all(from.path().join("models/m")).unwrap();
        std::fs::write(from.path().join("models/m/a.bin"), b"aaaa").unwrap();
        std::fs::write(from.path().join("models/m/b.bin"), b"bb").unwrap();
        // Already staged: must not be overwritten by the older copy.
        std::fs::create_dir_all(to.path().join("models/m")).unwrap();
        std::fs::write(to.path().join("models/m/b.bin"), b"NEW").unwrap();

        let moved = carry(
            from.path(),
            to.path(),
            &[
                "models/m/a.bin".to_string(),
                "models/m/b.bin".to_string(),
                "models/m/missing.bin".to_string(),
            ],
        )
        .await
        .expect("carry succeeds");

        assert_eq!(moved, 4, "only a.bin's bytes are counted");
        assert_eq!(
            std::fs::read(to.path().join("models/m/a.bin")).unwrap(),
            b"aaaa"
        );
        assert_eq!(
            std::fs::read(to.path().join("models/m/b.bin")).unwrap(),
            b"NEW",
            "an existing staged file wins"
        );
        assert!(
            !from.path().join("models/m/a.bin").exists(),
            "moved, not copied"
        );
    }
}
