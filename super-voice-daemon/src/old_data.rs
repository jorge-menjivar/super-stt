// SPDX-License-Identifier: GPL-3.0-only
//! Moving what Super STT and Super TTS kept before they were one product —
//! the backends each installed, and the voices cloned with Super TTS — into
//! Super Voice's data folder, on the first start that finds them.
//!
//! The folders are moved, not copied: a backend's holds its manifest, its
//! entrypoint, the record of which release is installed and the models it
//! downloaded, which can be gigabytes, and a voice's holds its recording. The
//! old and the new folder sit in the same data directory, so each move is a
//! rename.
//!
//! A folder already in the new place is never replaced. The old copy is left
//! where it was, and the log says so.
//!
//! A hidden folder is neither a backend nor a voice: it is the old product's
//! scratch space, like the folder its installer staged downloads in. It is
//! never moved. An empty one is removed, so the old folder can go too.
//!
//! A backend Super Voice cannot read is not moved either. In the new folder
//! discovery would skip it on every scan, and the Library would not show it,
//! so its models would fill the disk with no way to remove them from the app.
//! It stays in the old folder, where the user can find it.

use log::{info, warn};
use std::path::Path;

/// Why Super Voice cannot use the folder at a path, if it cannot. Checked
/// before a folder is moved.
pub type Usable = fn(&Path) -> Result<(), String>;

/// Why Super Voice cannot use the backend in `dir`, if it cannot: the rules
/// discovery holds an installed backend to.
///
/// # Errors
/// The reason, when its manifest cannot be read or breaks a runtime rule.
pub fn readable_backend(dir: &Path) -> Result<(), String> {
    use crate::stt_models::backends::manifest::{Manifest, validate_runtime};
    let manifest =
        Manifest::load_installed(dir).map_err(|e| format!("{:#}", anyhow::Error::from(e)))?;
    validate_runtime(&manifest).map_err(|e| format!("{e:#}"))
}

/// The products whose data Super Voice takes over, by their data folders'
/// names.
pub const OLD_PRODUCTS: [&str; 2] = ["super-stt", "super-tts"];

/// What a [`move_folders`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Moved {
    /// The folders now in the new place, by name.
    pub moved: Vec<String>,
    /// The folders left behind, because Super Voice cannot use them, the new
    /// place already had one of the same name, or the move failed.
    pub left: Vec<String>,
}

/// Move every folder in `from` that `usable` accepts into `into`, then remove
/// `from` if nothing is left in it. `what` is one of them, `backend` or
/// `voice`, for the log. Nothing to move, or no `from` at all, is not an
/// error.
///
/// # Errors
/// When `from` exists but cannot be read, or `into` cannot be created. A
/// folder that cannot be moved is left in place and reported in
/// [`Moved::left`], not returned as an error, so one bad folder does not keep
/// the others from moving.
pub fn move_folders(
    from: &Path,
    into: &Path,
    what: &str,
    usable: Usable,
) -> std::io::Result<Moved> {
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
        if shown.starts_with('.') {
            if std::fs::remove_dir(from.join(&name)).is_ok() {
                info!("Removed the empty {shown} from {}", from.display());
            }
            continue;
        }
        if let Err(reason) = usable(&from.join(&name)) {
            info!(
                "Leaving {what} {shown} in {}: Super Voice cannot use it: {reason}",
                from.display()
            );
            report.left.push(shown);
            continue;
        }
        let target = into.join(&name);
        if target.exists() {
            warn!(
                "Not moving {what} {shown} from {}: {} already has one",
                from.display(),
                into.display()
            );
            report.left.push(shown);
            continue;
        }
        match std::fs::rename(from.join(&name), &target) {
            Ok(()) => {
                info!("Moved {what} {shown} to {}", target.display());
                report.moved.push(shown);
            }
            Err(e) => {
                warn!("Could not move {what} {shown} from {}: {e}", from.display());
                report.left.push(shown);
            }
        }
    }

    // Only succeeds once the folder is empty, which is what is wanted: a
    // folder left behind stays where the user can find it.
    if std::fs::remove_dir(from).is_ok() {
        info!("Removed {}, now empty", from.display());
    }
    Ok(report)
}

/// The old `product`'s folder matching one of Super Voice's, beside it in the
/// data directory: `<data>/super-tts/voices` for `<data>/super-voice/voices`.
#[must_use]
pub fn old_product_dir(voice_dir: &Path, product: &str) -> Option<std::path::PathBuf> {
    Some(
        voice_dir
            .parent()?
            .parent()?
            .join(product)
            .join(voice_dir.file_name()?),
    )
}

#[cfg(test)]
mod tests {
    use super::{Moved, move_folders, old_product_dir, readable_backend};
    use std::path::Path;

    fn backend(root: &Path, name: &str, marker: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join("models")).unwrap();
        std::fs::write(dir.join("backend.toml"), marker).unwrap();
    }

    /// Every folder is one Super Voice can use.
    #[allow(clippy::unnecessary_wraps)]
    fn any(_: &Path) -> Result<(), String> {
        Ok(())
    }

    /// A Super STT backend, from before contracts named a product or a role.
    const SUPER_STT_MANIFEST: &str = r#"
[backend]
    source      = "github.com/x/whisper"
    name        = "Whisper"
    version     = "0.1.0"
    kind        = "subprocess"
    entrypoint  = "whisper"
    contract    = "v1"
    description = "Test backend."

[[models]]
    name                = "tiny"
    primary_language    = "en"
    supported_languages = ["en"]
    supported_devices   = ["cpu"]
"#;

    /// The same, with a key of text to speech's, as Super TTS's backends
    /// declared them: without a role, Super Voice reads the model as
    /// transcribing, and refuses the key.
    fn super_tts_manifest() -> String {
        SUPER_STT_MANIFEST.replace(
            "supported_devices   = [\"cpu\"]",
            "supported_devices   = [\"cpu\"]\n    max_input_chars     = 500",
        )
    }

    #[test]
    fn a_super_stt_backend_is_readable() {
        let home = tempfile::tempdir().unwrap();
        backend(home.path(), "app.super-stt.whisper", SUPER_STT_MANIFEST);
        assert_eq!(
            readable_backend(&home.path().join("app.super-stt.whisper")),
            Ok(())
        );
    }

    #[test]
    fn a_super_tts_backend_is_not_readable() {
        let home = tempfile::tempdir().unwrap();
        backend(home.path(), "app.super-tts.kokoro", &super_tts_manifest());
        let reason = readable_backend(&home.path().join("app.super-tts.kokoro")).unwrap_err();
        assert!(reason.contains("max_input_chars"), "{reason}");
    }

    /// A backend Super Voice cannot read stays in the old folder. Moved, it
    /// would be skipped on every scan and missing from the Library, with no
    /// way to remove it from the app.
    #[test]
    fn a_backend_super_voice_cannot_read_is_left_where_it_was() {
        let home = tempfile::tempdir().unwrap();
        let from = home.path().join("super-tts/backends");
        let into = home.path().join("super-voice/backends");
        backend(&from, "app.super-tts.kokoro", &super_tts_manifest());
        backend(&from, "app.super-stt.whisper", SUPER_STT_MANIFEST);

        let report = move_folders(&from, &into, "backend", readable_backend).unwrap();

        assert_eq!(
            report,
            Moved {
                moved: vec!["app.super-stt.whisper".into()],
                left: vec!["app.super-tts.kokoro".into()],
            }
        );
        assert!(from.join("app.super-tts.kokoro/backend.toml").is_file());
        assert!(!into.join("app.super-tts.kokoro").exists());
    }

    #[test]
    fn moves_every_backend_and_removes_the_empty_folder() {
        let home = tempfile::tempdir().unwrap();
        let from = home.path().join("super-stt/backends");
        let into = home.path().join("super-voice/backends");
        backend(&from, "app.super-stt.whisper", "whisper");
        backend(&from, "app.super-stt.voxtral", "voxtral");

        let report = move_folders(&from, &into, "backend", any).unwrap();

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

        let report = move_folders(&from, &into, "backend", any).unwrap();

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

    /// The installer's staging folder is no backend. Super Voice has its own,
    /// so moving it was refused with a warning on every start, and the old
    /// folder could never be removed.
    #[test]
    fn an_empty_staging_folder_is_removed_not_moved() {
        let home = tempfile::tempdir().unwrap();
        let from = home.path().join("super-tts/backends");
        let into = home.path().join("super-voice/backends");
        std::fs::create_dir_all(from.join(".staging")).unwrap();
        std::fs::create_dir_all(into.join(".staging")).unwrap();
        backend(&from, "app.super-tts.kokoro", "kokoro");

        let report = move_folders(&from, &into, "backend", any).unwrap();

        assert_eq!(
            report,
            Moved {
                moved: vec!["app.super-tts.kokoro".into()],
                left: vec![],
            }
        );
        assert!(!from.exists(), "the emptied folder is removed");
        assert!(into.join(".staging").is_dir(), "Super Voice's own is kept");
    }

    /// A hidden folder with something in it is left where it is, and is not
    /// reported as a backend left behind.
    #[test]
    fn a_staging_folder_in_use_is_left_alone() {
        let home = tempfile::tempdir().unwrap();
        let from = home.path().join("super-tts/backends");
        let into = home.path().join("super-voice/backends");
        std::fs::create_dir_all(from.join(".staging/partial")).unwrap();

        let report = move_folders(&from, &into, "backend", any).unwrap();

        assert_eq!(report, Moved::default());
        assert!(from.join(".staging/partial").is_dir());
    }

    #[test]
    fn nothing_to_move_is_not_an_error() {
        let home = tempfile::tempdir().unwrap();
        let report = move_folders(
            &home.path().join("super-stt/backends"),
            &home.path().join("super-voice/backends"),
            "backend",
            any,
        )
        .unwrap();
        assert_eq!(report, Moved::default());
        assert!(
            !home.path().join("super-voice/backends").exists(),
            "nothing is created when there was nothing to move"
        );
    }

    #[test]
    fn the_old_folders_are_beside_super_voice_s() {
        assert_eq!(
            old_product_dir(Path::new("/d/super-voice/backends"), "super-stt").as_deref(),
            Some(Path::new("/d/super-stt/backends"))
        );
        assert_eq!(
            old_product_dir(Path::new("/d/super-voice/voices"), "super-tts").as_deref(),
            Some(Path::new("/d/super-tts/voices"))
        );
    }

    /// Voices move the same way: one folder per voice, recording and all.
    #[test]
    fn moves_cloned_voices() {
        let home = tempfile::tempdir().unwrap();
        let from = home.path().join("super-tts/voices");
        let into = home.path().join("super-voice/voices");
        std::fs::create_dir_all(from.join("3f2a")).unwrap();
        std::fs::write(from.join("3f2a/clip.wav"), b"RIFF").unwrap();
        std::fs::write(from.join("3f2a/voice.json"), b"{}").unwrap();

        let report = move_folders(&from, &into, "voice", any).unwrap();

        assert_eq!(report.moved, vec!["3f2a".to_string()]);
        assert_eq!(std::fs::read(into.join("3f2a/clip.wav")).unwrap(), b"RIFF");
        assert!(!from.exists());
    }
}
