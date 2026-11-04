// SPDX-License-Identifier: GPL-3.0-only
//! macOS Notification Center, for a daemon running from inside
//! `Super STT.app`, by way of the app's own executable.
//!
//! Notification Center serves only an app bundle's own executable. The daemon
//! sits beside it in `Contents/MacOS` and has the app as its main bundle, and
//! still every request it makes through `UNUserNotificationCenter` fails as
//! "not allowed" (`UNErrorDomain` 1), whatever the user has chosen, without
//! `usernoted` logging anything about it. So the daemon runs the app's
//! executable in notify mode, `super-stt-app --notify <title> <body>`, which
//! posts as Super STT and exits; see `super-stt-app/src/core/app/macos/notifier.rs`.
//! The banner carries the app's name and icon, a click opens the app, and a
//! post the user has not allowed comes back as a failure.
//!
//! A bare daemon (`just run-daemon`) has no app executable to run, and posts
//! through `osascript` instead.

use anyhow::{Context as _, Result, anyhow, ensure};
use objc2_foundation::NSBundle;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The app's notify-mode flag; `notifier::FLAG` in super-stt-app.
const NOTIFY_FLAG: &str = "--notify";

/// How long the app gets to post. It answers in well under a second, or in
/// five seconds when it has to ask the user and they have not clicked; past
/// this the process is stuck, and the failure has to reach the user some
/// other way. This is awaited on the recording path.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(10);

/// The app's executable, the bundle's `CFBundleExecutable`.
const APP_EXECUTABLE: &str = "Contents/MacOS/super-stt-app";

/// The app's executable, when this daemon runs from inside the bundle.
///
/// The main bundle is the enclosing `.app` only when the daemon is in its
/// `Contents/MacOS`. From anywhere else it is the daemon's own directory,
/// which is no app.
///
/// Built from the bundle's path, not read off `executablePath`: for the main
/// bundle that names the running process — the daemon — whatever the
/// bundle's own executable is.
pub(crate) fn notifier() -> Option<PathBuf> {
    let bundle = PathBuf::from(NSBundle::mainBundle().bundlePath().to_string());
    let is_app = bundle
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("app"));
    let exe = bundle.join(APP_EXECUTABLE);
    (is_app && exe.is_file()).then_some(exe)
}

/// Post `summary` as the title and `body` as the text of a banner, through
/// the app at `notifier`.
///
/// # Errors
/// When the user has turned Super STT's notifications off or not yet
/// allowed them, when Notification Center refuses the post, or when the app
/// cannot be run or does not finish. Unlike the `osascript` path, a banner
/// that will not be shown is reported, so [`NotificationMethod::Auto`] can
/// fall back to typing.
///
/// [`NotificationMethod::Auto`]: super_stt_shared::models::notification_method::NotificationMethod::Auto
pub(crate) async fn post(notifier: &Path, summary: &str, body: &str) -> Result<()> {
    let child = tokio::process::Command::new(notifier)
        .arg(NOTIFY_FLAG)
        .arg(summary)
        .arg(body)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| {
            format!(
                "could not run {} to post a notification",
                notifier.display()
            )
        })?;

    let output = tokio::time::timeout(NOTIFY_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| anyhow!("the app did not post the notification within {NOTIFY_TIMEOUT:?}"))?
        .context("the app failed while posting a notification")?;
    let reason = String::from_utf8_lossy(&output.stderr);
    ensure!(
        output.status.success(),
        "{}",
        match reason.trim() {
            "" => format!("the app exited with {} while posting", output.status),
            reason => reason.to_string(),
        }
    );
    Ok(())
}
