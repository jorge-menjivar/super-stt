// SPDX-License-Identifier: GPL-3.0-only
//! Desktop notification delivery for synthesis failures.
//!
//! Uses the freedesktop Desktop Notifications interface
//! (`org.freedesktop.Notifications`), which every mainstream desktop provides —
//! GNOME, KDE Plasma, COSMIC, XFCE, MATE, Cinnamon, `LXQt` — as do the
//! standalone servers used on bare compositors (mako, dunst, swaync). One code
//! path covers all of them; nothing here is desktop-specific.
//!
//! What a bubble says is decided in [`crate::output::notice`], including how
//! backend-authored text is made safe to put in a body; this module only carries
//! it to the bus.

use crate::output::notice::Failure;
use anyhow::{Context, Result};
use log::{debug, info, warn};
use std::collections::HashMap;
use super_tts_shared::models::notification_method::NotificationMethod;
use zbus::Connection;
use zbus::zvariant::Value;

const NOTIFY_BUS: &str = "org.freedesktop.Notifications";
const NOTIFY_PATH: &str = "/org/freedesktop/Notifications";
const NOTIFY_IFACE: &str = "org.freedesktop.Notifications";

/// Sent as the notification's `app_name`, which is where the user learns who
/// this bubble is from. The summary is free to name the failure instead.
const APP_NAME: &str = "Super TTS";
/// Installed into `share/icons/hicolor/scalable/apps` by the justfile.
const APP_ICON: &str = "super-tts-app";
/// 0 = low, 1 = normal, 2 = critical.
const URGENCY_NORMAL: u8 = 1;
/// Let the notification server pick the timeout.
const EXPIRE_DEFAULT: i32 = -1;

/// Sends failure notices to the session's notification server.
///
/// `pub` (not `pub(crate)`) because it is held in a `pub` field on the `pub`
/// `SuperTTSDaemon`.
pub struct Notifier {
    inner: Inner,
    /// Id of the last notification sent, passed as `replaces_id` so repeated
    /// failures replace the previous bubble instead of stacking. The spec
    /// treats 0 as "do not replace".
    last_id: u32,
}

/// Every `(summary, body)` a [`Notifier::fake`] was asked to send.
#[cfg(test)]
type Sent = std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>;

enum Inner {
    /// Session-bus connection, established on first send and cached.
    Dbus(Option<Connection>),
    #[cfg(test)]
    Fake { fail: bool, sent: Sent },
}

impl Notifier {
    #[must_use]
    pub fn dbus() -> Self {
        Self {
            inner: Inner::Dbus(None),
            last_id: 0,
        }
    }

    /// Deliver `summary` and `body` as a desktop notification.
    ///
    /// # Errors
    /// Returns an error when no session bus is reachable, or when no
    /// notification server owns `org.freedesktop.Notifications`. Callers treat
    /// both the same way: there is nowhere to show a notification.
    ///
    /// # Panics
    /// Never in practice: the `expect` below only unwraps the connection slot
    /// this same call just populated a few lines above.
    pub async fn send(&mut self, summary: &str, body: &str) -> Result<()> {
        match &mut self.inner {
            Inner::Dbus(slot) => {
                if slot.is_none() {
                    *slot = Some(
                        Connection::session()
                            .await
                            .context("no session bus available for notifications")?,
                    );
                }
                let conn = slot.as_ref().expect("connection established above");
                let proxy = zbus::Proxy::new(conn, NOTIFY_BUS, NOTIFY_PATH, NOTIFY_IFACE)
                    .await
                    .context("could not build the notifications proxy")?;

                let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
                hints.insert("urgency", Value::U8(URGENCY_NORMAL));
                let actions: Vec<&str> = Vec::new();

                // Notify(app_name, replaces_id, app_icon, summary, body,
                //        actions, hints, expire_timeout) -> id
                let id: u32 = proxy
                    .call(
                        "Notify",
                        &(
                            APP_NAME,
                            self.last_id,
                            APP_ICON,
                            summary,
                            body,
                            actions,
                            hints,
                            EXPIRE_DEFAULT,
                        ),
                    )
                    .await
                    .context("no notification server answered on the session bus")?;

                self.last_id = id;
                debug!("Delivered failure notification (id {id})");
                Ok(())
            }
            #[cfg(test)]
            Inner::Fake { fail, sent } => {
                if *fail {
                    anyhow::bail!("fake notifier: delivery failed");
                }
                sent.lock()
                    .unwrap()
                    .push((summary.to_string(), body.to_string()));
                Ok(())
            }
        }
    }

    /// A notifier that records the `(summary, body)` of what it was asked to
    /// send, or fails every send when `fail` is true.
    #[cfg(test)]
    pub(crate) fn fake(fail: bool) -> (Self, Sent) {
        let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        (
            Self {
                inner: Inner::Fake {
                    fail,
                    sent: std::sync::Arc::clone(&sent),
                },
                last_id: 0,
            },
            sent,
        )
    }
}

/// Route a failure notice to the user through the configured channel.
///
/// Delivery can fail with no channel left — a bare compositor with no
/// notification server running — and that is not an error worth propagating:
/// the caller already learned about the failure from the response it got. It
/// goes to the log so the reason is still recoverable.
pub(crate) async fn deliver(
    method: NotificationMethod,
    notifier: &mut Notifier,
    failure: &Failure,
) {
    match method {
        NotificationMethod::Off => {
            info!(
                "Synthesis failure: {} — {} (surfacing disabled)",
                failure.summary, failure.body
            );
        }
        NotificationMethod::Auto => {
            if let Err(e) = notifier.send(failure.summary, &failure.body).await {
                warn!(
                    "Could not deliver failure notification ({} — {}): {e}",
                    failure.summary, failure.body
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::notice::Origin;

    /// A synthesis failure carrying a backend's reason — the shape the user
    /// hits most often, and the one that has both a summary and a body to check.
    fn backend_failure() -> Failure {
        Failure::synthesis_failed(Origin::Backend, "Could not reach the server (write_failed)")
    }

    #[tokio::test(start_paused = true)]
    async fn off_sends_nothing() {
        let (mut n, sent) = Notifier::fake(false);
        deliver(NotificationMethod::Off, &mut n, &backend_failure()).await;
        assert!(sent.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn auto_sends_the_summary_and_body() {
        let (mut n, sent) = Notifier::fake(false);
        deliver(
            NotificationMethod::Auto,
            &mut n,
            &Failure::could_not_open_output("Audio device disappeared mid-utterance"),
        )
        .await;

        assert_eq!(
            *sent.lock().unwrap(),
            vec![(
                "Could not play audio".to_string(),
                "Audio device disappeared mid-utterance".to_string()
            )]
        );
    }

    /// The bug this replaced: a bubble that named the app twice and the reason
    /// not at all.
    #[tokio::test(start_paused = true)]
    async fn the_bubble_carries_the_reason_and_not_a_second_app_name() {
        let (mut n, sent) = Notifier::fake(false);
        deliver(NotificationMethod::Auto, &mut n, &backend_failure()).await;

        let sent = sent.lock().unwrap().clone();
        let (summary, body) = sent.first().expect("one notification");
        assert_eq!(summary, "Synthesis failed");
        assert_eq!(
            body,
            "Backend error: Could not reach the server (write_failed)"
        );
        assert!(
            !summary.contains(APP_NAME) && !body.contains(APP_NAME),
            "the app name is the notification's own field, not text"
        );
    }

    /// A compositor with no notification server must not take the daemon down
    /// with it: an undeliverable notice is logged, not propagated.
    #[tokio::test(start_paused = true)]
    async fn a_failed_delivery_is_swallowed() {
        let (mut n, _sent) = Notifier::fake(true);
        deliver(
            NotificationMethod::Auto,
            &mut n,
            &Failure::no_model_loaded(),
        )
        .await;
    }
}
