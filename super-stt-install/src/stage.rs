// SPDX-License-Identifier: GPL-3.0-only
//! Staging: extract the release tarball, detect which components to
//! install, and build the manifest the root phase later applies verbatim.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use super_stt_registry_types::verify::{tar_budget_step, tar_entry_unsafe_reason, unpack_cap};

use crate::errors::InstallError;

/// Which of the three installable components are selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Components {
    pub daemon: bool,
    pub app: bool,
    pub applet: bool,
}

impl Components {
    /// The wire strings for the `complete` event's `components` field, in
    /// `["daemon", "app", "applet"]` order (only the selected subset).
    #[must_use]
    #[allow(clippy::trivially_copy_pass_by_ref)] // interface fixed by the design doc: `&self`
    pub fn names(&self) -> Vec<String> {
        let mut names = Vec::new();
        if self.daemon {
            names.push("daemon".to_string());
        }
        if self.app {
            names.push("app".to_string());
        }
        if self.applet {
            names.push("applet".to_string());
        }
        names
    }
}

/// An explicit `--component` CLI selection (`all|daemon|app|applet`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentSelection {
    All,
    Daemon,
    App,
    Applet,
}

/// Decide which components to install.
///
/// `explicit`, if given, wins outright — no detection runs. Otherwise: if
/// `<prefix>/bin` already has any `super-stt-*` binary, this is an update —
/// only the components already present are refreshed (the daemon always,
/// since it is the core piece every install has). Otherwise this is a fresh
/// install: daemon and app always, and the applet only when `cosmic_available`
/// (installing a COSMIC applet with no COSMIC panel to host it is useless).
#[must_use]
pub fn plan_components(
    explicit: Option<ComponentSelection>,
    prefix: &Path,
    cosmic_available: bool,
) -> Components {
    if let Some(selection) = explicit {
        return match selection {
            ComponentSelection::All => Components {
                daemon: true,
                app: true,
                applet: true,
            },
            ComponentSelection::Daemon => Components {
                daemon: true,
                ..Components::default()
            },
            ComponentSelection::App => Components {
                app: true,
                ..Components::default()
            },
            ComponentSelection::Applet => Components {
                applet: true,
                ..Components::default()
            },
        };
    }

    let installed: Vec<String> = std::fs::read_dir(prefix.join("bin"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    let is_update = installed.iter().any(|name| name.starts_with("super-stt-"));

    if is_update {
        Components {
            daemon: true,
            app: installed.iter().any(|name| name == "super-stt-app"),
            applet: installed
                .iter()
                .any(|name| name == "super-stt-cosmic-applet"),
        }
    } else {
        Components {
            daemon: true,
            app: true,
            applet: cosmic_available,
        }
    }
}

/// Unpack the release tarball at `tarball` (gzip'd tar) into `staging`,
/// creating `staging` if needed.
///
/// Two passes, mirroring the daemon's install-time extraction
/// (`super-stt-daemon/src/registry/install.rs`) and the indexer's publish-time
/// validation (`super-stt-indexer/src/assets.rs`) so a tarball that passes
/// one passes the other: the first pass rejects any entry
/// [`tar_entry_unsafe_reason`] flags (an absolute path, a `..` component, or a
/// symlink — a symlink entry would otherwise become a root-copied link target
/// once staged), *before* anything is written to disk; the second pass
/// unpacks while enforcing the archive-scaled [`unpack_cap`]/[`tar_budget_step`]
/// decompression-bomb budget.
///
/// # Errors
/// [`InstallError::InstallFailed`] if `tarball` cannot be opened, contains an
/// unsafe entry, exceeds the unpack budget, or cannot be unpacked into
/// `staging`.
pub fn extract_tarball(tarball: &Path, staging: &Path) -> Result<(), InstallError> {
    let fail = |what: &str, e: &dyn std::fmt::Display| {
        InstallError::InstallFailed(format!("{}: {what}: {e}", tarball.display()))
    };
    let total_cap = unpack_cap(
        std::fs::metadata(tarball)
            .map_err(|e| fail("stat", &e))?
            .len(),
    );

    // First pass: validate every entry without unpacking anything.
    {
        let file = std::fs::File::open(tarball).map_err(|e| fail("open", &e))?;
        let decoder = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);
        for entry in archive.entries().map_err(|e| fail("read", &e))? {
            let entry = entry.map_err(|e| fail("read entry", &e))?;
            let path = entry.path().map_err(|e| fail("read entry path", &e))?;
            let path_str = path.to_string_lossy();
            if let Some(reason) =
                tar_entry_unsafe_reason(&path_str, entry.header().entry_type().is_symlink())
            {
                return Err(InstallError::InstallFailed(format!(
                    "{}: unsafe tar entry: {reason}",
                    tarball.display()
                )));
            }
        }
    }

    // Second pass: unpack, enforcing the per-entry and total-output budgets.
    std::fs::create_dir_all(staging).map_err(|e| fail("create staging dir", &e))?;
    let file = std::fs::File::open(tarball).map_err(|e| fail("open", &e))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let mut total: u64 = 0;
    for entry in archive.entries().map_err(|e| fail("read", &e))? {
        let mut entry = entry.map_err(|e| fail("read entry", &e))?;
        total = tar_budget_step(entry.size(), total, total_cap).map_err(|reason| {
            InstallError::InstallFailed(format!("{}: {reason}", tarball.display()))
        })?;
        entry.unpack_in(staging).map_err(|e| fail("extract", &e))?;
    }
    Ok(())
}

/// The `stt` convenience wrapper — invokes `super-stt-cli` directly. Used by
/// keyboard shortcuts (e.g. Super+Space → `stt record --write`). Content is
/// exactly what `scripts/install-beta.sh` has installed to date.
#[must_use]
pub fn wrapper_script(prefix: &Path) -> String {
    format!(
        "#!/bin/bash\n\
         # Super STT convenience wrapper — invokes super-stt-cli directly.\n\
         # Used by keyboard shortcuts (e.g. Super+Space → \"stt record --write\").\n\
         exec \"{}/bin/super-stt-cli\" \"$@\"\n",
        prefix.display()
    )
}

/// One file the root phase copies into place: `source` (inside `staging`, or
/// the running installer's own path for the self-entry) → `dest` (an
/// absolute path under an allowed root), installed with permission `mode`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub source: PathBuf,
    pub dest: PathBuf,
    pub mode: u32,
}

/// The full set of files to install, built while still unprivileged so the
/// root phase only ever copies files it's already been told about — it never
/// reads the tarball, the network, or `$HOME`.
///
/// Deliberately carries no `staging_root`/`self_exe` fields: those are the
/// root phase's trust anchors for [`crate::root_phase::apply_manifest`]'s
/// source-containment check, and a value read back out of this same,
/// untrusted JSON would let a crafted manifest defend against itself (e.g.
/// claim its own `staging_root` is `/`). The root phase derives both from
/// argv/the OS instead — see `root_phase::run`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub entries: Vec<ManifestEntry>,
}

/// Every `resources/super-stt-cosmic-applet-*.desktop` file in `staging`,
/// sorted for deterministic manifest ordering.
fn applet_desktop_files(staging: &Path) -> Vec<PathBuf> {
    let resources = staging.join("resources");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&resources)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.starts_with("super-stt-cosmic-applet-") && n.ends_with(".desktop")
            })
        })
        .collect();
    files.sort();
    files
}

/// A required source is missing from `staging` — the tarball is malformed or
/// doesn't carry what `components` asked for.
fn required_entry(
    staging: &Path,
    rel: &str,
    dest: PathBuf,
    mode: u32,
) -> Result<ManifestEntry, InstallError> {
    let source = staging.join(rel);
    if !source.exists() {
        return Err(InstallError::InstallFailed(format!(
            "staging missing {rel}"
        )));
    }
    Ok(ManifestEntry { source, dest, mode })
}

/// Build the manifest for `components`, resolving every source against
/// `staging` and every destination against `prefix` (`unit_dir` for the
/// systemd unit). `self_exe` is the running installer's own binary — every
/// install/update re-stages a copy of itself at `<prefix>/bin/super-stt-install`
/// so a later `--root-phase` re-run and in-app updates keep working. The root
/// phase recognizes this entry by comparing its `source` against its own
/// `std::env::current_exe()` at apply time — nothing here needs to (or
/// should) tell it which entry that is.
///
/// # Errors
/// [`InstallError::InstallFailed`] when a required source for a selected
/// component is missing from `staging` — caught here, before escalation, so a
/// broken tarball never prompts for a root password.
#[allow(clippy::trivially_copy_pass_by_ref)] // interface fixed by the design doc: `&Components`
pub fn build_manifest(
    staging: &Path,
    prefix: &Path,
    unit_dir: &Path,
    components: &Components,
    self_exe: &Path,
) -> Result<Manifest, InstallError> {
    let bin = prefix.join("bin");
    let mut entries = vec![ManifestEntry {
        source: self_exe.to_path_buf(),
        dest: bin.join("super-stt-install"),
        mode: 0o755,
    }];

    if components.daemon {
        for name in ["super-stt-daemon", "super-stt-cli", "super-stt-consent"] {
            entries.push(required_entry(staging, name, bin.join(name), 0o755)?);
        }

        // The wrapper has no source in the tarball — generate it now and
        // stage it as a real file, so the root phase only ever copies files.
        let generated_dir = staging.join("generated");
        std::fs::create_dir_all(&generated_dir).map_err(|e| {
            InstallError::InstallFailed(format!("create {}: {e}", generated_dir.display()))
        })?;
        let wrapper_path = generated_dir.join("stt");
        std::fs::write(&wrapper_path, wrapper_script(prefix)).map_err(|e| {
            InstallError::InstallFailed(format!("write {}: {e}", wrapper_path.display()))
        })?;
        entries.push(ManifestEntry {
            source: wrapper_path,
            dest: bin.join("stt"),
            mode: 0o755,
        });

        entries.push(required_entry(
            staging,
            "systemd/super-stt.service",
            unit_dir.join("super-stt.service"),
            0o644,
        )?);
    }

    if components.app {
        entries.push(required_entry(
            staging,
            "super-stt-app",
            bin.join("super-stt-app"),
            0o755,
        )?);
        entries.push(required_entry(
            staging,
            "resources/super-stt-app.desktop",
            prefix.join("share/applications/super-stt-app.desktop"),
            0o644,
        )?);
        entries.push(required_entry(
            staging,
            "resources/icons/hicolor/scalable/apps/super-stt-app.svg",
            prefix.join("share/icons/hicolor/scalable/apps/super-stt-app.svg"),
            0o644,
        )?);
        // Metainfo is optional upstream (older tarballs may not carry it):
        // skip the entry rather than fail if it's absent.
        let metainfo = staging.join("resources/super-stt-app.metainfo.xml");
        if metainfo.exists() {
            entries.push(ManifestEntry {
                source: metainfo,
                dest: prefix.join("share/metainfo/super-stt-app.metainfo.xml"),
                mode: 0o644,
            });
        }
    }

    if components.applet {
        entries.push(required_entry(
            staging,
            "super-stt-cosmic-applet",
            bin.join("super-stt-cosmic-applet"),
            0o755,
        )?);
        for path in applet_desktop_files(staging) {
            let name = path
                .file_name()
                .expect("filtered on file_name above")
                .to_owned();
            entries.push(ManifestEntry {
                source: path,
                dest: prefix.join("share/applications").join(name),
                mode: 0o644,
            });
        }
        entries.push(required_entry(
            staging,
            "resources/icons/hicolor/scalable/apps/super-stt-cosmic-applet.svg",
            prefix.join("share/icons/hicolor/scalable/apps/super-stt-cosmic-applet.svg"),
            0o644,
        )?);
    }

    Ok(Manifest { entries })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// A fresh, empty per-test temp directory (per-pid, plus a per-call
    /// counter so parallel tests in this binary never collide).
    fn test_dir() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("sstt-install-stage-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A temp dir with each of `paths` touched as an empty file (parent dirs
    /// created as needed). Used as a fake `prefix` for detection tests.
    fn mktree(paths: &[&str]) -> PathBuf {
        let dir = test_dir();
        for p in paths {
            let full = dir.join(p);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, b"").unwrap();
        }
        dir
    }

    /// The full tarball layout (binaries at root, `systemd/`, `resources/**`)
    /// in a temp dir, as a fake `staging` tree.
    fn fake_staging() -> PathBuf {
        fake_staging_without("")
    }

    /// Like [`fake_staging`], but omits the entry whose basename is `missing`.
    fn fake_staging_without(missing: &str) -> PathBuf {
        let dir = test_dir();
        let files = [
            "super-stt-daemon",
            "super-stt-cli",
            "super-stt-consent",
            "super-stt-app",
            "super-stt-cosmic-applet",
            "systemd/super-stt.service",
            "resources/super-stt-app.desktop",
            "resources/icons/hicolor/scalable/apps/super-stt-app.svg",
            "resources/super-stt-app.metainfo.xml",
            "resources/super-stt-cosmic-applet-full.desktop",
            "resources/super-stt-cosmic-applet-left.desktop",
            "resources/super-stt-cosmic-applet-right.desktop",
            "resources/icons/hicolor/scalable/apps/super-stt-cosmic-applet.svg",
        ];
        for f in files {
            if !missing.is_empty() && std::path::Path::new(f).file_name().unwrap() == missing {
                continue;
            }
            let full = dir.join(f);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, b"").unwrap();
        }
        dir
    }

    #[test]
    fn detection_update_mode_only_updates_whats_present() {
        let prefix = mktree(&["bin/super-stt-daemon", "bin/super-stt-app"]); // helper: temp dir + touch files
        let c = plan_components(None, &prefix, true);
        assert!(c.daemon && c.app && !c.applet);
    }

    #[test]
    fn detection_fresh_install_takes_all_gated_on_cosmic() {
        let prefix = mktree(&[]); // empty bin/
        let c = plan_components(None, &prefix, false);
        assert!(c.daemon && c.app && !c.applet);
        let c = plan_components(None, &prefix, true);
        assert!(c.applet);
    }

    #[test]
    fn explicit_selection_wins() {
        let prefix = mktree(&["bin/super-stt-app"]);
        let c = plan_components(Some(ComponentSelection::Daemon), &prefix, true);
        assert!(c.daemon && !c.app && !c.applet);
    }

    #[test]
    fn manifest_covers_selected_components_with_modes() {
        let staging = fake_staging(); // helper: full tarball layout in a temp dir
        let m = build_manifest(
            &staging,
            std::path::Path::new("/usr/local"),
            std::path::Path::new("/usr/lib/systemd/user"),
            &Components {
                daemon: true,
                app: true,
                applet: false,
            },
            std::path::Path::new("/proc/self/exe"),
        )
        .unwrap();
        let dests: Vec<String> = m
            .entries
            .iter()
            .map(|e| e.dest.display().to_string())
            .collect();
        assert!(dests.contains(&"/usr/local/bin/super-stt-daemon".to_string()));
        assert!(dests.contains(&"/usr/local/bin/stt".to_string()));
        assert!(dests.contains(&"/usr/lib/systemd/user/super-stt.service".to_string()));
        assert!(dests.contains(&"/usr/local/share/applications/super-stt-app.desktop".to_string()));
        assert!(dests.contains(&"/usr/local/bin/super-stt-install".to_string()));
        assert!(!dests.iter().any(|d| d.contains("cosmic-applet")));
        let unit = m
            .entries
            .iter()
            .find(|e| e.dest.ends_with("super-stt.service"))
            .unwrap();
        assert_eq!(unit.mode, 0o644);
        let stt = m
            .entries
            .iter()
            .find(|e| e.dest.ends_with("bin/stt"))
            .unwrap();
        assert_eq!(stt.mode, 0o755);
        // The generated wrapper's staged content is exactly the known script.
        let content = std::fs::read_to_string(&stt.source).unwrap();
        assert!(content.starts_with("#!/bin/bash\n"));
        assert!(content.contains("exec \"/usr/local/bin/super-stt-cli\" \"$@\""));
    }

    #[test]
    fn manifest_missing_required_source_errors_before_escalation() {
        let staging = fake_staging_without("super-stt-app");
        let err = build_manifest(
            &staging,
            std::path::Path::new("/usr/local"),
            std::path::Path::new("/usr/lib/systemd/user"),
            &Components {
                daemon: false,
                app: true,
                applet: false,
            },
            std::path::Path::new("/proc/self/exe"),
        )
        .unwrap_err();
        assert!(matches!(err, crate::errors::InstallError::InstallFailed(_)));
    }

    #[test]
    fn tarball_round_trip() {
        let tmp = test_dir();
        let tree = tmp.join("tree");
        std::fs::create_dir_all(tree.join("systemd")).unwrap();
        std::fs::write(tree.join("super-stt-daemon"), b"bin").unwrap();
        std::fs::write(tree.join("systemd/super-stt.service"), b"[Unit]").unwrap();
        let tgz = tmp.join("t.tar.gz");
        {
            let f = std::fs::File::create(&tgz).unwrap();
            let enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
            let mut tar = tar::Builder::new(enc);
            tar.append_dir_all(".", &tree).unwrap();
            tar.into_inner().unwrap().finish().unwrap();
        }
        let out = tmp.join("out");
        extract_tarball(&tgz, &out).unwrap();
        assert_eq!(std::fs::read(out.join("super-stt-daemon")).unwrap(), b"bin");
        assert_eq!(
            std::fs::read(out.join("systemd/super-stt.service")).unwrap(),
            b"[Unit]"
        );
    }

    #[test]
    fn extract_tarball_rejects_a_symlink_entry() {
        // A symlink entry in the tarball would otherwise become a
        // root-copied link target once staged and manifest-installed — the
        // same exploit class as a dangling symlink at the install
        // destination, but smuggled in from the archive side.
        let tmp = test_dir();
        let tgz = tmp.join("evil.tar.gz");
        {
            let f = std::fs::File::create(&tgz).unwrap();
            let enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
            let mut tar = tar::Builder::new(enc);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            tar.append_link(&mut header, "evil-link", "/etc/passwd")
                .unwrap();
            tar.into_inner().unwrap().finish().unwrap();
        }
        let out = tmp.join("out");
        assert!(extract_tarball(&tgz, &out).is_err());
        assert!(!out.join("evil-link").exists());
    }
}
