// SPDX-License-Identifier: GPL-3.0-only
//! What the settings app does as the main executable of `Super STT.app`:
//! register the bundle's `LaunchAgent`s, ask to show notifications, and say
//! where the daemon's agent stands while the daemon cannot be reached.
//!
//! None of it applies to a bare binary in `target/` (`just run-app`). That
//! has no bundle for `ServiceManagement` to find the agents in, and none for
//! Notification Center to attribute banners to, which raises an exception
//! there rather than returning an error.

use block2::RcBlock;
use cosmic::iced::futures::{SinkExt as _, Stream};
use objc2::runtime::Bool;
use objc2_foundation::{NSBundle, NSError};
use objc2_user_notifications::{UNAuthorizationOptions, UNUserNotificationCenter};
use std::time::Duration;
use super_stt_shared::launch_agents::{Agent, Status};

use crate::ui::messages::{Message, ShellMessage};

/// How often [`daemon_agent`] looks again. Only while the daemon is
/// unreachable, which is when the connection page shows what it found.
const AGENT_POLL: Duration = Duration::from_secs(3);

/// Whether this process is the executable of an app bundle.
pub(crate) fn in_bundle() -> bool {
    let bundle = NSBundle::mainBundle();
    bundle.bundleIdentifier().is_some()
        && std::path::Path::new(&bundle.bundlePath().to_string())
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
}

/// Register each agent that has never been registered, which starts it and
/// runs it at every login.
///
/// This is what installs the daemon for someone who drags Super STT into
/// Applications and opens it, so it runs on every launch rather than once.
/// An agent switched off in Login Items is left alone: that was the user's
/// choice, and only they can undo it.
///
/// Blocking — `ServiceManagement` answers over XPC — so it runs off the UI
/// thread.
pub(crate) fn register_agents() {
    for agent in Agent::ALL {
        if agent.status() != Status::NotRegistered {
            continue;
        }
        match agent.register() {
            Ok(()) => log::info!("registered {}", agent.label()),
            Err(e) => log::warn!("{e:#}"),
        }
    }
}

/// Ask to show notifications.
///
/// The daemon's failures are posted by this executable in notify mode (see
/// `notifier`), so the permission is this bundle's. Asked from here so the
/// prompt comes up while the user is looking at Super STT, rather than the
/// first time a dictation fails. Once the user has answered, the framework
/// answers for them.
pub(crate) fn request_notification_permission() {
    let block = RcBlock::new(|granted: Bool, error: *mut NSError| {
        // SAFETY: the framework passes either null or an `NSError` that is
        // live for the length of the call.
        match unsafe { error.as_ref() } {
            Some(e) => log::warn!(
                "could not ask to show notifications: {}",
                e.localizedDescription()
            ),
            None => log::debug!("notifications allowed: {}", granted.as_bool()),
        }
    });
    UNUserNotificationCenter::currentNotificationCenter()
        .requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert, &block);
}

/// The daemon agent's status now, and again every [`AGENT_POLL`], for as
/// long as the subscription runs.
pub(crate) fn daemon_agent() -> impl Stream<Item = Message> {
    cosmic::iced::stream::channel(1, async |mut output| {
        loop {
            let Ok(status) = tokio::task::spawn_blocking(|| Agent::Daemon.status()).await else {
                break;
            };
            let message = Message::Shell(ShellMessage::DaemonAgent(status));
            if output.send(message).await.is_err() {
                break;
            }
            tokio::time::sleep(AGENT_POLL).await;
        }
    })
}
