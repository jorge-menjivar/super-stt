// SPDX-License-Identifier: GPL-3.0-only
//! `super-stt-app --notify <title> <body>`: post one banner to Notification
//! Center as Super STT, then exit. The daemon runs this; see
//! `super-stt-daemon/src/output/notification_center.rs`.
//!
//! Notification Center serves only an app bundle's own executable. The
//! daemon lives beside this binary in `Contents/MacOS` and has the app as its
//! main bundle, and still every request it makes fails as "not allowed"
//! (`UNErrorDomain` 1), whatever the user has chosen, without `usernoted`
//! logging anything about it. So the daemon hands each banner to a
//! short-lived process of this executable.
//!
//! That process has to present itself as an application, or Notification
//! Center refuses it as well ("Failed to find or validate client"). It sets up
//! the shared `NSApplication` and finishes launching it — with no Dock icon,
//! no windows and no event loop — then posts and exits. None of libcosmic
//! starts.

use anyhow::{Context as _, Result, anyhow, bail};
use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::runtime::Bool;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::{NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent,
    UNNotificationRequest, UNNotificationSettings, UNUserNotificationCenter,
};
use std::ptr::NonNull;
use std::sync::mpsc;
use std::time::Duration;

/// The flag that selects this mode. The daemon passes the same string.
pub(crate) const FLAG: &str = "--notify";

/// The identifier every banner is posted under. Reusing one replaces the
/// banner already showing instead of stacking a new one beside it — what
/// `replaces_id` does for the daemon on Linux.
const REQUEST_ID: &str = "super-stt";

/// How long to wait for each answer from Notification Center. A settings
/// query or a post is answered at once. A permission request is answered
/// when the user clicks, which this cannot wait for: the daemon is waiting on
/// this process to learn whether to fall back to typing.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// Whether this process was started in notify mode.
pub(crate) fn requested() -> bool {
    std::env::args_os().nth(1).is_some_and(|arg| arg == FLAG)
}

/// Post the banner the command line describes, and exit: 0 once Notification
/// Center has it, 1 with the reason on stderr when it refused, 2 for a
/// malformed command line.
pub(crate) fn run() -> ! {
    let args: Vec<String> = std::env::args_os()
        .skip(2)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let [title, body] = args.as_slice() else {
        eprintln!("usage: super-stt-app {FLAG} <title> <body>");
        std::process::exit(2)
    };
    if let Err(e) = post(title, body) {
        eprintln!("{e:#}");
        std::process::exit(1)
    }
    std::process::exit(0)
}

fn post(title: &str, body: &str) -> Result<()> {
    let mtm = MainThreadMarker::new().context("notify mode must run on the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
    app.finishLaunching();

    let center = UNUserNotificationCenter::currentNotificationCenter();
    match authorization_status(&center)? {
        UNAuthorizationStatus::Denied => {
            bail!("notifications from Super STT are turned off in System Settings › Notifications")
        }
        // The app asks when it opens, so this is reached only by a banner
        // posted before Super STT was ever opened.
        UNAuthorizationStatus::NotDetermined if !request_authorization(&center)? => {
            bail!("notifications from Super STT were not allowed")
        }
        _ => {}
    }
    add_request(&center, title, body)
}

fn authorization_status(center: &UNUserNotificationCenter) -> Result<UNAuthorizationStatus> {
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
        // SAFETY: the framework passes a live settings object for the length
        // of the call.
        let _ = tx.send(unsafe { settings.as_ref() }.authorizationStatus());
    });
    center.getNotificationSettingsWithCompletionHandler(&block);
    reply(&rx, "reading notification settings")
}

fn request_authorization(center: &UNUserNotificationCenter) -> Result<bool> {
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |granted: Bool, error: *mut NSError| {
        let _ = tx.send(error_from(error).map_or(Ok(granted.as_bool()), Err));
    });
    center.requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert, &block);
    reply(&rx, "asking to show notifications")?
}

fn add_request(center: &UNUserNotificationCenter, title: &str, body: &str) -> Result<()> {
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(REQUEST_ID),
        &content,
        None,
    );

    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |error: *mut NSError| {
        let _ = tx.send(error_from(error).map_or(Ok(()), Err));
    });
    center.addNotificationRequest_withCompletionHandler(&request, Some(&block));
    reply(&rx, "posting the notification")?
}

/// A completion block's answer, bounded by [`REPLY_TIMEOUT`].
fn reply<T>(rx: &mpsc::Receiver<T>, doing: &str) -> Result<T> {
    rx.recv_timeout(REPLY_TIMEOUT)
        .with_context(|| format!("Notification Center did not answer while {doing}"))
}

/// The error a completion block was handed, if any.
fn error_from(error: *mut NSError) -> Option<anyhow::Error> {
    // SAFETY: the framework passes either null or an `NSError` that is live
    // for the length of the call.
    let error = unsafe { error.as_ref() }?;
    Some(anyhow!("{}", error.localizedDescription()))
}
