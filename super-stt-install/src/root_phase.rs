// SPDX-License-Identifier: GPL-3.0-only
//! The escalated (`--root-phase`) step: apply a pre-built [`crate::stage::Manifest`]
//! verbatim. Runs under `pkexec`, which strips the environment — this module
//! must never read `$HOME`, other env vars, or the network.

use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use crate::errors::InstallError;
use crate::stage::Manifest;

/// The only prefixes an entry's `dest` may resolve under. Trailing slashes
/// are load-bearing: a plain string-prefix check against `"/usr/local/"`
/// correctly rejects a sibling directory like `/usr/local2/...` that would
/// otherwise slip past a check against `"/usr/local"` (no slash).
pub const ALLOWED_ROOTS: &[&str] = &["/usr/local/", "/usr/lib/systemd/user/"];

/// Absolute path to the icon-cache refresh tool. A bare `Command::new(name)`
/// would resolve `name` against `$PATH` — an implicit environment read this
/// module's own contract forbids — so this is pinned and only ever invoked
/// after confirming the file exists at exactly this path.
const GTK_UPDATE_ICON_CACHE: &str = "/usr/bin/gtk-update-icon-cache";

/// Validate that `dest` is safe to write as root: absolute, free of `..`
/// components, and under one of `roots`.
///
/// # Errors
/// [`InstallError::InstallFailed`] describing which check failed.
pub fn validate_dest(dest: &Path, roots: &[&str]) -> Result<(), InstallError> {
    if !dest.is_absolute() {
        return Err(InstallError::InstallFailed(format!(
            "{}: not an absolute path",
            dest.display()
        )));
    }
    if dest.components().any(|c| c == Component::ParentDir) {
        return Err(InstallError::InstallFailed(format!(
            "{}: contains a `..` component",
            dest.display()
        )));
    }
    let dest_str = dest.to_string_lossy();
    if roots.iter().any(|root| dest_str.starts_with(root)) {
        Ok(())
    } else {
        Err(InstallError::InstallFailed(format!(
            "{}: outside the allowed install roots",
            dest.display()
        )))
    }
}

/// Reject any mode carrying setuid, setgid, or the sticky bit. A manifest
/// entry only ever needs standard permission bits (0o755/0o644 today); a
/// setuid-root binary is not something any legitimate entry should — or
/// needs to — install, so it's refused outright rather than trusted.
///
/// # Errors
/// [`InstallError::InstallFailed`] when `mode & 0o7000 != 0`.
fn validate_mode(mode: u32) -> Result<(), InstallError> {
    if mode & 0o7000 != 0 {
        return Err(InstallError::InstallFailed(format!(
            "mode {mode:#o} sets setuid/setgid/sticky bits — refusing"
        )));
    }
    Ok(())
}

/// Whether `source` is the installer's own running binary — the one
/// deliberate exception to [`validate_source`]'s containment check.
///
/// This is intentionally **not** derived from anything the manifest carries:
/// [`crate::stage::Manifest`] has no `self_exe` field precisely so a crafted
/// manifest can never claim an arbitrary path is "the installer" and walk
/// through this exception. Instead it asks the OS directly —
/// `std::env::current_exe()` on the *escalated* process is, by construction,
/// the binary `pkexec` re-invoked (`<current_exe> --root-phase`) — and
/// canonicalizes both sides before comparing, so a non-canonical (but
/// equivalent) path string doesn't spuriously mismatch.
///
/// Fails closed: if `current_exe()` or canonicalizing either path errors,
/// this returns `false` rather than treating the failure as a pass.
fn is_self_exe(source: &Path) -> bool {
    let Ok(current_exe) = std::env::current_exe() else {
        return false;
    };
    let Ok(real_source) = std::fs::canonicalize(source) else {
        return false;
    };
    let Ok(real_current_exe) = std::fs::canonicalize(&current_exe) else {
        return false;
    };
    real_source == real_current_exe
}

/// Require `source` to be free of `..` components (mirroring
/// `validate_dest`), and either the installer's own running binary
/// ([`is_self_exe`]) or under `staging_root`.
///
/// `staging_root` must come from outside the manifest — see `run`, which
/// derives it from the manifest file's own location on disk (argv), never
/// from the manifest's JSON contents. A value read back out of the same
/// untrusted document it's meant to constrain (e.g. a manifest claiming its
/// own `staging_root` is `/`) would let a crafted manifest defend against
/// itself, which is exactly the bug this function exists to close.
///
/// # Errors
/// [`InstallError::InstallFailed`] when `source` contains a `..` component,
/// or is neither the running executable nor under `staging_root`.
fn validate_source(source: &Path, staging_root: &Path) -> Result<(), InstallError> {
    if source.components().any(|c| c == Component::ParentDir) {
        return Err(InstallError::InstallFailed(format!(
            "{}: contains a `..` component",
            source.display()
        )));
    }
    if is_self_exe(source) {
        return Ok(());
    }
    if source.starts_with(staging_root) {
        Ok(())
    } else {
        Err(InstallError::InstallFailed(format!(
            "{}: source is outside the staging root {}",
            source.display(),
            staging_root.display()
        )))
    }
}

/// After `create_dir_all(parent)`, confirm `parent` resolves to exactly
/// itself with no symlink anywhere in the chain. `validate_dest` is purely
/// lexical (it never touches the filesystem), so a manifest whose `dest`
/// lexically validates fine could still have an ancestor directory — e.g.
/// `/usr/local/bin` itself — replaced with a symlink pointing outside every
/// allowed root; `create_dir_all` would silently no-op against it (the
/// symlink already "is" a directory as far as `metadata` is concerned) and
/// every subsequent write would land wherever that symlink points. Comparing
/// the canonicalized path against the lexical one catches exactly that,
/// without needing to canonicalize (and thus depend on the existence of) the
/// allowed roots themselves.
///
/// # Errors
/// [`InstallError::InstallFailed`] if `parent` cannot be resolved, or
/// resolves to anything other than itself.
fn ensure_no_symlinked_ancestor(parent: &Path) -> Result<(), InstallError> {
    let real = std::fs::canonicalize(parent)
        .map_err(|e| InstallError::InstallFailed(format!("resolve {}: {e}", parent.display())))?;
    if real == parent {
        Ok(())
    } else {
        Err(InstallError::InstallFailed(format!(
            "{}: resolves to {} — refusing to follow a symlinked ancestor",
            parent.display(),
            real.display()
        )))
    }
}

/// Apply every entry in `manifest`: validate first (dest, mode, and
/// source — so a manifest with one bad entry touches nothing at all), then
/// for each entry create its parent directory (rejecting a symlinked
/// ancestor), stage the new content beside `dest` under a private temp name
/// with `mode` already set, and atomically rename it into place.
///
/// `staging_root` is the trust anchor for [`validate_source`] — the caller
/// (`run`) derives it from where the manifest file itself lives on disk
/// (argv), never from the manifest's own JSON; see that function's docs for
/// why. Every entry's `source` must resolve under it, except the installer's
/// own running binary ([`is_self_exe`]).
///
/// The rename — rather than unlinking (or worse, writing through) `dest`
/// directly — is deliberate: `rename(2)` replaces whatever is at `dest`,
/// *including a dangling symlink*, without ever dereferencing it. A direct
/// `dest.exists()` check follows symlinks (so a dangling one reports
/// `false`), which would skip the unlink and let a plain write or
/// `set_permissions` follow the link and land on — and chmod — whatever it
/// points to, anywhere on disk. Atomic rename also keeps GNU `install`'s
/// unlink-then-create semantics: a binary that's currently running keeps
/// executing its old (now-unlinked) inode instead of erroring or corrupting
/// a live mapping.
///
/// # Errors
/// [`InstallError::InstallFailed`] on the first entry that fails validation
/// or I/O.
pub fn apply_manifest(
    manifest: &Manifest,
    roots: &[&str],
    staging_root: &Path,
) -> Result<(), InstallError> {
    for entry in &manifest.entries {
        validate_dest(&entry.dest, roots)?;
        validate_mode(entry.mode)?;
        validate_source(&entry.source, staging_root)?;
    }
    for entry in &manifest.entries {
        let parent = entry.dest.parent().ok_or_else(|| {
            InstallError::InstallFailed(format!(
                "{}: has no parent directory",
                entry.dest.display()
            ))
        })?;
        std::fs::create_dir_all(parent).map_err(|e| {
            InstallError::InstallFailed(format!("create {}: {e}", parent.display()))
        })?;
        ensure_no_symlinked_ancestor(parent)?;

        let tmp_name = format!(
            ".super-stt-install.{}.{}.tmp",
            std::process::id(),
            entry
                .dest
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("entry")
        );
        let tmp_path = parent.join(tmp_name);
        // Best-effort: clear a stale temp file left behind by a previous
        // failed run under the same (unlikely, but not impossible) name.
        let _ = std::fs::remove_file(&tmp_path);

        let write_result = std::fs::copy(&entry.source, &tmp_path)
            .map_err(|e| {
                InstallError::InstallFailed(format!(
                    "copy {} -> {}: {e}",
                    entry.source.display(),
                    tmp_path.display()
                ))
            })
            .and_then(|_| {
                std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(entry.mode))
                    .map_err(|e| {
                        InstallError::InstallFailed(format!("chmod {}: {e}", tmp_path.display()))
                    })
            })
            .and_then(|()| {
                std::fs::rename(&tmp_path, &entry.dest).map_err(|e| {
                    InstallError::InstallFailed(format!(
                        "rename {} -> {}: {e}",
                        tmp_path.display(),
                        entry.dest.display()
                    ))
                })
            });
        if write_result.is_err() {
            let _ = std::fs::remove_file(&tmp_path);
        }
        write_result?;
    }
    Ok(())
}

/// Read the manifest at `manifest_path`, capped at
/// [`super_stt_registry_types::verify::MAX_MANIFEST_BYTES`] (checked via
/// `metadata` *before* reading, so an oversized file is never actually read
/// into memory), and parse it.
///
/// # Errors
/// [`InstallError::InstallFailed`] if `manifest_path` cannot be stat'd,
/// exceeds the size cap, cannot be read, or does not parse as a [`Manifest`].
fn read_manifest(manifest_path: &Path) -> Result<Manifest, InstallError> {
    let len = std::fs::metadata(manifest_path)
        .map_err(|e| InstallError::InstallFailed(format!("stat {}: {e}", manifest_path.display())))?
        .len();
    if len > super_stt_registry_types::verify::MAX_MANIFEST_BYTES {
        return Err(InstallError::InstallFailed(format!(
            "{}: manifest is {len} bytes, exceeding the {}-byte cap",
            manifest_path.display(),
            super_stt_registry_types::verify::MAX_MANIFEST_BYTES
        )));
    }
    let text = std::fs::read_to_string(manifest_path).map_err(|e| {
        InstallError::InstallFailed(format!("read {}: {e}", manifest_path.display()))
    })?;
    serde_json::from_str(&text)
        .map_err(|e| InstallError::InstallFailed(format!("parse {}: {e}", manifest_path.display())))
}

/// The staging directory that owns `manifest_path` — i.e. the directory the
/// manifest file itself lives in. Derived from `manifest_path` (which comes
/// from argv: the path the escalator passed on the command line when it
/// invoked `<current_exe> --root-phase <manifest_path>`), **never** from the
/// manifest's own JSON contents — that's the whole point: a second,
/// unprivileged user who cannot write into the real staging directory cannot
/// place a manifest there either, so this anchor can't be forged by crafting
/// manifest content alone. Canonicalized so a symlinked ancestor can't
/// smuggle a different real directory in under the same lexical path.
///
/// # Errors
/// [`InstallError::InstallFailed`] if `manifest_path` has no parent
/// directory, or that directory cannot be resolved.
fn staging_root_of(manifest_path: &Path) -> Result<PathBuf, InstallError> {
    let parent = manifest_path.parent().ok_or_else(|| {
        InstallError::InstallFailed(format!(
            "{}: has no parent directory",
            manifest_path.display()
        ))
    })?;
    std::fs::canonicalize(parent)
        .map_err(|e| InstallError::InstallFailed(format!("resolve {}: {e}", parent.display())))
}

/// The escalated step, run as root under `pkexec`. Reads and parses the
/// manifest at `manifest_path`, applies it, and best-effort refreshes the
/// icon cache. Never reads `$HOME`, other environment variables, or the
/// network — `pkexec` strips the environment, so this must not depend on it.
///
/// The two trust anchors [`apply_manifest`]'s source-containment check
/// needs — the staging root and "is this the installer itself" — are both
/// derived here from outside the manifest (argv and `std::env::current_exe()`
/// respectively, via [`staging_root_of`] and [`is_self_exe`]), never read
/// back out of the manifest's own JSON.
///
/// Returns a process exit code: `0` on success, `1` on any [`InstallError`]
/// (logged to stderr).
#[must_use]
pub fn run(manifest_path: &Path) -> u8 {
    let outcome = read_manifest(manifest_path).and_then(|manifest| {
        let staging_root = staging_root_of(manifest_path)?;
        apply_manifest(&manifest, ALLOWED_ROOTS, &staging_root)
    });

    if let Err(e) = outcome {
        eprintln!("error: {e}");
        return 1;
    }

    // Best-effort: a missing gtk-update-icon-cache or a failing refresh is
    // not an install failure, just a stale icon until the next refresh. The
    // tool is invoked by its absolute path only — never by bare name, which
    // would implicitly consult `$PATH`.
    if Path::new(GTK_UPDATE_ICON_CACHE).is_file() {
        let _ = std::process::Command::new(GTK_UPDATE_ICON_CACHE)
            .args(["-f", "-t", "/usr/local/share/icons/hicolor"])
            .status();
    }

    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stage::ManifestEntry;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// A fresh, empty per-test temp directory (per-pid, plus a per-call
    /// counter so parallel tests in this binary never collide).
    fn test_dir() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("sstt-install-root-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rejects_escapes_and_foreign_paths() {
        let roots = &["/usr/local/", "/usr/lib/systemd/user/"];
        assert!(validate_dest(Path::new("/usr/local/bin/x"), roots).is_ok());
        assert!(validate_dest(Path::new("/usr/lib/systemd/user/x.service"), roots).is_ok());
        assert!(validate_dest(Path::new("/etc/passwd"), roots).is_err());
        assert!(validate_dest(Path::new("/usr/local/../lib/x"), roots).is_err());
        assert!(validate_dest(Path::new("relative/x"), roots).is_err());
        assert!(validate_dest(Path::new("/usr/local2/bin/x"), roots).is_err()); // prefix, not path-component, trap
    }

    #[test]
    fn apply_manifest_installs_with_modes_and_replaces_existing() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = test_dir(); // per-pid temp dir helper
        let root = tmp.join("fake-usr-local");
        let src = tmp.join("staged-bin");
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(&src, b"new-binary").unwrap();
        let dest = root.join("bin/tool");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"old-binary").unwrap();
        let roots_owned = format!("{}/", root.display());
        let manifest = Manifest {
            entries: vec![ManifestEntry {
                source: src,
                dest: dest.clone(),
                mode: 0o755,
            }],
        };
        apply_manifest(&manifest, &[roots_owned.as_str()], &tmp).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"new-binary");
        assert_eq!(
            std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn apply_manifest_refuses_out_of_root_entry_without_touching_it() {
        let tmp = test_dir();
        let victim = tmp.join("victim");
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(&victim, b"untouched").unwrap();
        let src = tmp.join("payload");
        std::fs::write(&src, b"evil").unwrap();
        let manifest = Manifest {
            entries: vec![ManifestEntry {
                source: src,
                dest: victim.clone(),
                mode: 0o644,
            }],
        };
        assert!(apply_manifest(&manifest, &["/usr/local/"], &tmp).is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"untouched");
    }

    #[test]
    fn apply_manifest_refuses_to_follow_a_dangling_symlink_at_dest() {
        // C1: a dangling symlink at `dest` pointing outside the injected
        // root must never be followed — the file it points to must not be
        // created, and `dest` itself must end up holding the real content
        // (the symlink replaced, not chased).
        use std::os::unix::fs::symlink;
        let tmp = test_dir();
        let root = tmp.join("fake-usr-local");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let outside_target = tmp.join("outside-target"); // the attacker's real target
        let dest = root.join("bin/tool");
        symlink(&outside_target, &dest).unwrap(); // dangling: outside_target doesn't exist
        let src = tmp.join("staged-bin");
        std::fs::write(&src, b"new-binary").unwrap();
        let roots_owned = format!("{}/", root.display());
        let manifest = Manifest {
            entries: vec![ManifestEntry {
                source: src,
                dest: dest.clone(),
                mode: 0o755,
            }],
        };
        apply_manifest(&manifest, &[roots_owned.as_str()], &tmp).unwrap();
        assert!(
            !outside_target.exists(),
            "must never write through the dangling symlink to its target"
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"new-binary");
        assert!(
            !std::fs::symlink_metadata(&dest)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the symlink at dest must have been replaced by the real file"
        );
    }

    #[test]
    fn apply_manifest_refuses_a_symlinked_ancestor_directory() {
        // The symlinked-PARENT variant: `dest`'s lexical path validates
        // fine, but its parent directory is itself a symlink escaping the
        // allowed root.
        use std::os::unix::fs::symlink;
        let tmp = test_dir();
        let root = tmp.join("fake-usr-local");
        std::fs::create_dir_all(&root).unwrap();
        let outside_dir = tmp.join("outside-bin");
        std::fs::create_dir_all(&outside_dir).unwrap();
        symlink(&outside_dir, root.join("bin")).unwrap();
        let dest = root.join("bin/tool");
        let src = tmp.join("staged-bin");
        std::fs::write(&src, b"evil").unwrap();
        let roots_owned = format!("{}/", root.display());
        let manifest = Manifest {
            entries: vec![ManifestEntry {
                source: src,
                dest: dest.clone(),
                mode: 0o644,
            }],
        };
        assert!(apply_manifest(&manifest, &[roots_owned.as_str()], &tmp).is_err());
        assert!(!outside_dir.join("tool").exists());
    }

    #[test]
    fn apply_manifest_rejects_setuid_mode_bits() {
        let tmp = test_dir();
        let root = tmp.join("fake-usr-local");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let src = tmp.join("staged-bin");
        std::fs::write(&src, b"binary").unwrap();
        let dest = root.join("bin/tool");
        let roots_owned = format!("{}/", root.display());
        let manifest = Manifest {
            entries: vec![ManifestEntry {
                source: src,
                dest: dest.clone(),
                mode: 0o4755, // setuid
            }],
        };
        assert!(apply_manifest(&manifest, &[roots_owned.as_str()], &tmp).is_err());
        assert!(!dest.exists());
    }

    #[test]
    fn apply_manifest_rejects_source_outside_caller_supplied_staging_root() {
        // Nothing in the manifest can widen the root: `staging_root` is
        // supplied by the caller (here, the test), not read from JSON — the
        // manifest doesn't even have a field for it any more.
        let tmp = test_dir();
        let root = tmp.join("fake-usr-local");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let staging = tmp.join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        let src_outside = tmp.join("not-in-staging"); // a sibling of staging, not under it
        std::fs::write(&src_outside, b"evil").unwrap();
        let dest = root.join("bin/tool");
        let roots_owned = format!("{}/", root.display());
        let manifest = Manifest {
            entries: vec![ManifestEntry {
                source: src_outside,
                dest: dest.clone(),
                mode: 0o755,
            }],
        };
        assert!(apply_manifest(&manifest, &[roots_owned.as_str()], &staging).is_err());
        assert!(!dest.exists());
    }

    #[test]
    fn apply_manifest_rejects_source_with_a_parent_dir_component() {
        let tmp = test_dir();
        let root = tmp.join("fake-usr-local");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let staging = tmp.join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(tmp.join("escape"), b"evil").unwrap();
        // Lexically escapes `staging` via `..`, mirroring the `dest` `..`
        // check `validate_dest` already applies.
        let src = staging.join("../escape");
        let dest = root.join("bin/tool");
        let roots_owned = format!("{}/", root.display());
        let manifest = Manifest {
            entries: vec![ManifestEntry {
                source: src,
                dest: dest.clone(),
                mode: 0o755,
            }],
        };
        assert!(apply_manifest(&manifest, &[roots_owned.as_str()], &staging).is_err());
        assert!(!dest.exists());
    }

    #[test]
    fn apply_manifest_allows_the_self_exe_entry_via_current_exe() {
        // The one deliberate exception to source containment, proven with
        // the real mechanism: `source` is this very test binary's own path
        // (`std::env::current_exe()`), and `staging_root` deliberately does
        // NOT contain it — the exception must fire via `is_self_exe`, not
        // via staging-root containment.
        let tmp = test_dir();
        let root = tmp.join("fake-usr-local");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let staging = tmp.join("staging"); // does not contain current_exe()
        std::fs::create_dir_all(&staging).unwrap();
        let self_exe = std::env::current_exe().unwrap();
        let dest = root.join("bin/super-stt-install");
        let roots_owned = format!("{}/", root.display());
        let manifest = Manifest {
            entries: vec![ManifestEntry {
                source: self_exe.clone(),
                dest: dest.clone(),
                mode: 0o755,
            }],
        };
        apply_manifest(&manifest, &[roots_owned.as_str()], &staging).unwrap();
        assert_eq!(
            std::fs::metadata(&dest).unwrap().len(),
            std::fs::metadata(&self_exe).unwrap().len(),
            "the copied file must match the real running binary"
        );
    }

    #[test]
    fn run_rejects_an_oversized_manifest_without_reading_it() {
        // A valid, empty-entries manifest padded with insignificant trailing
        // whitespace past the size cap: `serde_json` happily parses (and
        // `apply_manifest` would happily no-op) this if the cap didn't
        // reject it first, so this only passes once the size check actually
        // runs ahead of the parse — a malformed-JSON manifest would fail
        // either way and wouldn't prove the cap fired.
        let tmp = test_dir();
        let manifest_path = tmp.join("manifest.json");
        let padding =
            " ".repeat((super_stt_registry_types::verify::MAX_MANIFEST_BYTES + 1) as usize);
        let json = format!(r#"{{"entries":[]}}{padding}"#);
        std::fs::write(&manifest_path, json).unwrap();
        assert_eq!(run(&manifest_path), 1);
    }
}
