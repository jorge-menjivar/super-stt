// SPDX-License-Identifier: GPL-3.0-only
//! Self-update check orchestration: forge query, event, notification.

use crate::daemon::types::SuperSTTDaemon;
use super_stt_registry_types::forge::Forge;
use super_stt_shared::models::notification_method::NotificationMethod;
use super_stt_shared::models::protocol::DaemonStatusEvent;
use super_stt_shared::models::self_update::SelfUpdateStatus;

impl SuperSTTDaemon {
    /// Run a self-update check, publish `UpdateAvailable` when a not-yet-seen
    /// version turns up, and send a desktop notification for it. Used by both
    /// the periodic background task and `POST /update/check`.
    ///
    /// The notification fires once per version per daemon run
    /// ([`crate::self_update::SelfUpdateChecker::should_notify`]). The startup check is the first
    /// check of every run, so a daemon coming up with an update waiting always
    /// announces it; the daily checks behind it in the same run do not.
    pub async fn run_self_update_check_and_notify(&self) -> SelfUpdateStatus {
        let optin = self.config.read().await.update.beta_optin;
        let before = self.self_update.status().await;
        let client = super_stt_forge::client(Forge::Github);
        let (status, did_check) = self.self_update.run_check(client.as_ref(), optin).await;

        // A coalesced call (`did_check == false`) didn't perform the check
        // itself — the overlapping caller that did already ran this same
        // block for this result. `before` was snapshotted before either
        // caller's check completed, so skipping entirely here (rather than
        // trusting `before`) is what keeps two overlapping calls from both
        // publishing the event and both notifying for the same version
        // (task review round 1, Important finding).
        if did_check && status.update_available {
            let newly = !before.update_available || before.latest_version != status.latest_version;
            if newly && let Some(tag) = &status.latest_version {
                self.events
                    .publish_daemon_status(DaemonStatusEvent::UpdateAvailable {
                        latest_version: tag.clone(),
                    });
            }
            if let Some(tag) = status.latest_version.clone()
                && self.self_update.should_notify(&tag).await
            {
                let method = self.config.read().await.transcription.notification_method;
                if matches!(method, NotificationMethod::Dbus | NotificationMethod::Auto) {
                    let mut notifier = self.notifier.lock().await;
                    let sent = notifier
                        .send_with_actions(
                            &format!("Super STT {tag} is available"),
                            "Open Super STT to install the update.",
                            &[(
                                crate::output::notification::OPEN_APP_ACTION_KEY,
                                "Open Super STT",
                            )],
                        )
                        .await;
                    let conn = notifier.connection();
                    drop(notifier);
                    if let Ok(id) = sent {
                        self.self_update.record_notified(&tag).await;
                        // The bubble carries an "Open Super STT" action; a
                        // click on it (or the bubble itself, on servers that
                        // bind the default action) should bring the app up.
                        // Spawned so the check path isn't blocked waiting on
                        // a click that may never come.
                        if let Some(conn) = conn {
                            tokio::spawn(async move {
                                Self::wait_for_update_notification_click(conn, id).await;
                            });
                        }
                    }
                } else {
                    // Off/Typed: typing an update notice into the focused
                    // window would be hostile; log only, and don't record —
                    // switching methods later should still notify once.
                    log::info!("Update {tag} available (notification method: {method})");
                }
            }
        }
        status
    }

    /// Wait for the user to click the "Open Super STT" action on the update
    /// notification `id`, then launch the app. The app is single-instance
    /// (its `org.freedesktop.DbusActivation` interface), so launching it
    /// while it is already running activates and focuses the existing window
    /// instead of opening a second one.
    async fn wait_for_update_notification_click(conn: zbus::Connection, id: u32) {
        let Some(key) = crate::output::notification::Notifier::wait_for_action(&conn, id).await
        else {
            return;
        };
        if key != crate::output::notification::OPEN_APP_ACTION_KEY {
            return;
        }
        if let Err(e) = tokio::process::Command::new("super-stt-app").spawn() {
            log::warn!("failed to launch Super STT app from update notification: {e}");
        }
    }
}

#[cfg(test)]
#[path = "self_update_handlers_tests.rs"]
mod tests;
