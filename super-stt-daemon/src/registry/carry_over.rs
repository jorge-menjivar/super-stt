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
///
/// `Manifest::parse` does not enforce destination uniqueness across models,
/// so the same destination can legally appear more than once. When it does
/// with conflicting URLs, the destination maps to `None`: we cannot tell
/// which URL a file on disk (if any) actually came from, so treating it as
/// unrecognized forces a re-download rather than risking the wrong file
/// being carried over silently. The same destination declared twice with the
/// *same* URL is not a conflict and still maps to `Some(url)`.
fn declared(m: &Manifest) -> HashMap<&str, Option<&str>> {
    let mut out: HashMap<&str, Option<&str>> = HashMap::new();
    for model in &m.models {
        for f in &model.files {
            let dest = f.destination.as_str();
            let url = f.url.as_str();
            out.entry(dest)
                .and_modify(|existing| {
                    if *existing != Some(url) {
                        *existing = None;
                    }
                })
                .or_insert(Some(url));
        }
    }
    out
}

/// Destinations declared unambiguously by both manifests with the same URL,
/// sorted so the result is stable for logging and tests.
///
/// A destination whose declaration is ambiguous in either manifest (see
/// [`declared`]) never survives, even if one of its conflicting URLs happens
/// to match.
///
/// Every destination is validated as a safe relative path by `Manifest::parse`,
/// so callers may join these onto a directory without escaping it — `carry`
/// re-validates anyway, since it is not limited to receiving this function's
/// output.
#[must_use]
pub fn survivors(old: &Manifest, new: &Manifest) -> Vec<String> {
    let old_files = declared(old);
    let mut out: Vec<String> = declared(new)
        .into_iter()
        .filter_map(|(dest, url)| {
            let url = url?;
            (old_files.get(dest) == Some(&Some(url))).then(|| dest.to_string())
        })
        .collect();
    out.sort();
    out
}

/// Move each of `destinations` from `from_dir` into `to_dir`, returning the
/// total bytes moved.
///
/// Each destination must be a safe relative path: it is joined onto both
/// `from_dir` and `to_dir` below, so a `..` component would let it climb out
/// of either. `survivors` only ever returns manifest-declared destinations,
/// which `Manifest::parse` already validates this way, but `carry` is not
/// limited to that input, so it re-checks every destination itself before
/// touching the filesystem.
///
/// A destination missing under `from_dir` is skipped, and one that already
/// exists under `to_dir` is left alone — the staged copy is the newer one.
/// Both directories live under the backends directory, so these are
/// same-filesystem renames: constant-time, with no multi-gigabyte copy.
///
/// # Errors
/// Returns an `io::Error` if a destination is not a safe relative path, a
/// directory cannot be created, or a rename fails.
pub async fn carry(
    from_dir: &Path,
    to_dir: &Path,
    destinations: &[String],
) -> std::io::Result<u64> {
    // Joined onto both directories below, so none may climb out of either.
    // The manifest parser guards `[[models.files]].destination` this way;
    // `carry` reaches the same join and gets the same guard, checked for
    // every destination up front so a bad entry mutates nothing.
    for dest in destinations {
        if !super_stt_registry_types::is_safe_relative_path(dest) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("refusing to carry over unsafe destination: {dest}"),
            ));
        }
    }
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
        manifest_with_models(&[files])
    }

    /// Like `manifest_with`, but emits one `[[models]]` block per entry in
    /// `models`, so a destination can be declared more than once across
    /// separate models (to exercise the conflicting-URL case).
    fn manifest_with_models(models: &[&[(&str, &str)]]) -> Manifest {
        let blocks = models
            .iter()
            .enumerate()
            .map(|(i, files)| {
                let entries = files
                    .iter()
                    .map(|(url, dest)| format!("{{ url = \"{url}\", destination = \"{dest}\" }}"))
                    .collect::<Vec<_>>()
                    .join(",\n            ");
                format!(
                    r#"
[[models]]
    name               = "m{i}"
    primary_language   = "en"
    supported_languages = ["en"]
    supported_devices  = ["cpu"]
    files = [
            {entries}
    ]
"#
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
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
{blocks}
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

    #[test]
    fn a_destination_declared_twice_with_conflicting_urls_does_not_survive() {
        // The old manifest ambiguously declares "shared.bin" across two
        // models with different URLs, and unambiguously declares "z.bin";
        // the new manifest declares both destinations unambiguously,
        // "shared.bin" matching one of the two old URLs. The ambiguous old
        // declaration must still block "shared.bin", while "z.bin" — an
        // ordinary, unambiguous match — survives.
        let old = manifest_with_models(&[
            &[
                ("https://h/shared-v1.bin", "models/m/shared.bin"),
                ("https://h/z.bin", "models/m/z.bin"),
            ],
            &[("https://h/shared-v2.bin", "models/m/shared.bin")],
        ]);
        let new = manifest_with_models(&[&[
            ("https://h/shared-v1.bin", "models/m/shared.bin"),
            ("https://h/z.bin", "models/m/z.bin"),
        ]]);
        assert_eq!(
            survivors(&old, &new),
            vec!["models/m/z.bin".to_string()],
            "shared.bin's old declaration is ambiguous, so it cannot survive"
        );
    }

    #[test]
    fn a_destination_declared_twice_with_the_same_url_is_not_a_conflict() {
        let files: &[(&str, &str)] = &[
            ("https://h/z.bin", "models/m/z.bin"),
            ("https://h/a.bin", "models/m/a.bin"),
        ];
        // "a.bin" and "z.bin" are each declared identically by two separate
        // models in both manifests; the repeat must not read as a conflict.
        let repeat: &[(&str, &str)] = &[("https://h/a.bin", "models/m/a.bin")];
        let old = manifest_with_models(&[files, repeat]);
        let new = manifest_with_models(&[files, repeat]);
        assert_eq!(
            survivors(&old, &new),
            vec!["models/m/a.bin".to_string(), "models/m/z.bin".to_string()],
            "both destinations survive, sorted, pinning `survivors`' sort"
        );
    }

    #[tokio::test]
    async fn carry_refuses_a_traversing_destination() {
        let from = tempfile::tempdir().unwrap();
        let to = tempfile::tempdir().unwrap();
        let from_nested = from.path().join("nested");
        std::fs::create_dir_all(&from_nested).unwrap();
        // If the guard were missing, `from_nested.join("../escaped.bin")`
        // would resolve to this path — still inside our sandbox, so we can
        // assert it was never touched.
        std::fs::write(from.path().join("escaped.bin"), b"secret").unwrap();

        let err = carry(&from_nested, to.path(), &["../escaped.bin".to_string()])
            .await
            .expect_err("a traversing destination must be refused, not joined");

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            from.path().join("escaped.bin").exists(),
            "the file outside `from_nested` must be untouched, not moved"
        );
        assert!(!to.path().join("escaped.bin").exists());
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
