// SPDX-License-Identifier: GPL-3.0-only
//! Moving the backends the speech-to-text daemon installed before it was Super
//! Voice into Super Voice's backends folder, on the first start that finds
//! them.
//!
//! The folders are moved, not copied: each holds a backend's manifest, its
//! entrypoint, the record of which release is installed and the models it
//! downloaded, which can be gigabytes. The old and the new folder sit in the
//! same data directory, so each move is a rename.
//!
//! A backend already in the new folder is never replaced. The old copy is
//! left where it was, and the log says so.

use log::{info, warn};
use std::path::Path;

/// What a [`move_backends`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Moved {
    /// The backends now in the new folder, by folder name.
    pub moved: Vec<String>,
    /// The backends left behind, because the new folder already had one of
    /// the same name or the move failed.
    pub left: Vec<String>,
}

/// Move every backend folder in `from` into `into`, then remove `from` if
/// nothing is left in it. Nothing to move, or no `from` at all, is not an
/// error.
///
/// # Errors
/// When `from` exists but cannot be read, or `into` cannot be created. A
/// backend that cannot be moved is left in place and reported in
/// [`Moved::left`], not returned as an error, so one bad folder does not keep
/// the others from moving.
pub fn move_backends(from: &Path, into: &Path) -> std::io::Result<Moved> {
    let entries = match std::fs::read_dir(from) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Moved::default()),
        Err(e) => return Err(e),
    };
    std::fs::create_dir_all(into)?;

    let mut report = Moved::default();
    let mut names: Vec<_> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.file_name())
        .collect();
    // Sorted so the log, and a test, read the same on every run.
    names.sort();
    for name in names {
        let shown = name.to_string_lossy().into_owned();
        let target = into.join(&name);
        if target.exists() {
            warn!(
                "Not moving backend {shown} from {}: {} already has one",
                from.display(),
                into.display()
            );
            report.left.push(shown);
            continue;
        }
        match std::fs::rename(from.join(&name), &target) {
            Ok(()) => {
                info!("Moved backend {shown} to {}", target.display());
                report.moved.push(shown);
            }
            Err(e) => {
                warn!(
                    "Could not move backend {shown} from {}: {e}",
                    from.display()
                );
                report.left.push(shown);
            }
        }
    }

    // Only succeeds once the folder is empty, which is what is wanted: a
    // backend left behind keeps its folder where the user can find it.
    if std::fs::remove_dir(from).is_ok() {
        info!("Removed {}, now empty", from.display());
    }
    Ok(report)
}

/// Super STT's backends folder, beside Super Voice's in the data directory:
/// `<data>/super-stt/backends` for `<data>/super-voice/backends`.
#[must_use]
pub fn super_stt_backends_dir(voice_backends: &Path) -> Option<std::path::PathBuf> {
    Some(
        voice_backends
            .parent()?
            .parent()?
            .join("super-stt")
            .join("backends"),
    )
}

#[cfg(test)]
mod tests {
    use super::{Moved, move_backends, super_stt_backends_dir};
    use std::path::Path;

    fn backend(root: &Path, name: &str, marker: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join("models")).unwrap();
        std::fs::write(dir.join("backend.toml"), marker).unwrap();
    }

    #[test]
    fn moves_every_backend_and_removes_the_empty_folder() {
        let home = tempfile::tempdir().unwrap();
        let from = home.path().join("super-stt/backends");
        let into = home.path().join("super-voice/backends");
        backend(&from, "app.super-stt.whisper", "whisper");
        backend(&from, "app.super-stt.voxtral", "voxtral");

        let report = move_backends(&from, &into).unwrap();

        assert_eq!(
            report,
            Moved {
                moved: vec![
                    "app.super-stt.voxtral".into(),
                    "app.super-stt.whisper".into()
                ],
                left: vec![],
            }
        );
        assert_eq!(
            std::fs::read_to_string(into.join("app.super-stt.whisper/backend.toml")).unwrap(),
            "whisper",
            "the folder moved whole"
        );
        assert!(into.join("app.super-stt.voxtral/models").is_dir());
        assert!(!from.exists(), "the emptied folder is removed");
    }

    /// A backend the new folder already has wins: the old one is not written
    /// over, and stays where the user can find it.
    #[test]
    fn never_replaces_a_backend_already_moved() {
        let home = tempfile::tempdir().unwrap();
        let from = home.path().join("super-stt/backends");
        let into = home.path().join("super-voice/backends");
        backend(&from, "app.super-stt.qwen", "old");
        backend(&into, "app.super-stt.qwen", "new");

        let report = move_backends(&from, &into).unwrap();

        assert_eq!(report.left, vec!["app.super-stt.qwen".to_string()]);
        assert_eq!(
            std::fs::read_to_string(into.join("app.super-stt.qwen/backend.toml")).unwrap(),
            "new"
        );
        assert!(
            from.join("app.super-stt.qwen").is_dir(),
            "the old copy stays"
        );
    }

    #[test]
    fn nothing_to_move_is_not_an_error() {
        let home = tempfile::tempdir().unwrap();
        let report = move_backends(
            &home.path().join("super-stt/backends"),
            &home.path().join("super-voice/backends"),
        )
        .unwrap();
        assert_eq!(report, Moved::default());
        assert!(
            !home.path().join("super-voice/backends").exists(),
            "nothing is created when there was nothing to move"
        );
    }

    #[test]
    fn super_stt_s_folder_is_beside_super_voice_s() {
        assert_eq!(
            super_stt_backends_dir(Path::new("/d/super-voice/backends")).as_deref(),
            Some(Path::new("/d/super-stt/backends"))
        );
    }
}
