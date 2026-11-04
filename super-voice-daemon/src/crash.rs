// SPDX-License-Identifier: GPL-3.0-only
//! What the daemon leaves behind when it panics, and the notice the next start
//! gives.
//!
//! A panic aborts the whole process (`panic = "abort"`): both features stop at
//! once, and the service manager starts the daemon again. Nothing survives the
//! abort to say what happened, so the hook says it first. It logs the panic —
//! where it happened, which feature that code belongs to, and a backtrace — and
//! leaves a note in the daemon's state folder. The next start finds the note,
//! tells the user the daemon restarted after a crash, and removes it.

use std::path::{Path, PathBuf};

/// Where the note goes: `<state>/super-voice/crashed`.
#[must_use]
pub fn note_path() -> PathBuf {
    super_stt_shared::paths::state_dir().join("crashed")
}

/// Which feature the code at `file` belongs to, as the log names it: text to
/// speech's lives under `src/tts/`; everything else is speech to text's or
/// the daemon's, which the path does not tell apart.
fn feature_of(file: &str) -> &'static str {
    if file.contains("/tts/") {
        "text to speech"
    } else {
        "speech to text or the daemon"
    }
}

/// Install the hook that logs a panic and leaves the note at `note`.
///
/// It runs before the abort, on the panicking thread, so it does nothing that
/// can block for long: one log line, one small file.
pub fn install_panic_hook(note: PathBuf) {
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map_or_else(|| "an unknown place".to_string(), ToString::to_string);
        let feature = info
            .location()
            .map_or("speech to text or the daemon", |l| feature_of(l.file()));
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "no message".to_string());
        let thread = std::thread::current();
        let thread = thread.name().unwrap_or("unnamed");
        let backtrace = std::backtrace::Backtrace::force_capture();
        log::error!(
            "Panicked in {feature} code at {location}, on thread {thread}: {message}\n{backtrace}"
        );
        let summary = format!(
            "{} {feature}: {message} at {location}\n",
            chrono::Utc::now().to_rfc3339()
        );
        if let Some(dir) = note.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&note, summary);
    }));
}

/// Take the note a crash left at `note`: its contents, with the note removed,
/// or `None` when the last run did not crash.
///
/// Removed before the notice is shown, so a crash while showing it is not
/// announced twice.
#[must_use]
pub fn take_note(note: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(note).ok()?;
    if let Err(e) = std::fs::remove_file(note) {
        log::warn!("Could not remove the crash note {}: {e}", note.display());
    }
    Some(contents)
}

impl crate::daemon::types::SuperSTTDaemon {
    /// Tell the user the daemon restarted after a crash, unless they turned
    /// notifications off. Called once, at the start that found the note.
    pub async fn announce_crash_restart(&self) {
        use super_stt_shared::models::notification_method::NotificationMethod;
        let method = self.config.read().await.notification_method;
        if !matches!(
            method,
            NotificationMethod::Desktop | NotificationMethod::Auto
        ) {
            return;
        }
        let sent = self
            .notifier
            .lock()
            .await
            .send(
                "Restarted after a crash",
                "Super Voice stopped unexpectedly and started again. Its log has the details.",
            )
            .await;
        if let Err(e) = sent {
            log::warn!("Could not show the crash notice: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{feature_of, take_note};

    #[test]
    fn a_note_is_taken_once() {
        let dir = tempfile::tempdir().unwrap();
        let note = dir.path().join("crashed");
        std::fs::write(&note, "2026-11-04T12:00:00Z text to speech: boom\n").unwrap();

        assert_eq!(
            take_note(&note).as_deref(),
            Some("2026-11-04T12:00:00Z text to speech: boom\n")
        );
        assert!(!note.exists(), "the note is removed");
        assert_eq!(take_note(&note), None, "a second start has nothing to say");
    }

    #[test]
    fn no_note_means_no_crash() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(take_note(&dir.path().join("crashed")), None);
    }

    /// The notice goes out like any other the daemon sends, and not at all
    /// when the user turned notifications off.
    #[tokio::test]
    async fn the_restart_is_announced_unless_notifications_are_off() {
        use crate::output::notification::Notifier;
        use super_stt_shared::models::notification_method::NotificationMethod;

        for (method, shown) in [
            (NotificationMethod::Auto, 1),
            (NotificationMethod::Desktop, 1),
            (NotificationMethod::Off, 0),
        ] {
            let mut daemon = crate::daemon::types::test_daemon().await;
            daemon.config.write().await.notification_method = method;
            let (notifier, sent) = Notifier::fake(false);
            daemon.notifier = std::sync::Arc::new(tokio::sync::Mutex::new(notifier));

            daemon.announce_crash_restart().await;

            let sent = sent.lock().unwrap();
            assert_eq!(sent.len(), shown, "{method:?}");
            if let Some((summary, _)) = sent.first() {
                assert_eq!(summary, "Restarted after a crash");
            }
        }
    }

    #[test]
    fn the_feature_is_read_off_the_path() {
        assert_eq!(
            feature_of("super-voice-daemon/src/tts/daemon/speech.rs"),
            "text to speech"
        );
        assert_eq!(
            feature_of("super-voice-daemon/src/stt_models/wasm/mod.rs"),
            "speech to text or the daemon"
        );
    }
}
