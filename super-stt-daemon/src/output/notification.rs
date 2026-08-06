// SPDX-License-Identifier: GPL-3.0-only
//! Desktop notification delivery for recording failures.
//!
//! Uses the freedesktop Desktop Notifications interface
//! (`org.freedesktop.Notifications`), which every mainstream desktop provides —
//! GNOME, KDE Plasma, COSMIC, XFCE, MATE, Cinnamon, `LXQt` — as do the
//! standalone servers used on bare compositors (mako, dunst, swaync). One code
//! path covers all of them; nothing here is desktop-specific.
//!
//! Notification bodies carry only the fixed constants from [`crate::output::notice`],
//! never backend error text. Backends are explicitly untrusted (audit 2 Tier 3
//! #8) and notification servers may render limited markup in the body, so the
//! same rule that governs typing governs this channel.

use crate::output::typer::Typer;
use anyhow::{Context, Result};
use log::{debug, info, warn};
use std::collections::HashMap;
use super_stt_shared::models::notification_method::NotificationMethod;
use zbus::Connection;
use zbus::zvariant::Value;

const NOTIFY_BUS: &str = "org.freedesktop.Notifications";
const NOTIFY_PATH: &str = "/org/freedesktop/Notifications";
const NOTIFY_IFACE: &str = "org.freedesktop.Notifications";

const APP_NAME: &str = "Super STT";
/// Installed into `share/icons/hicolor/scalable/apps` by the justfile.
const APP_ICON: &str = "super-stt-app";
const SUMMARY: &str = "Super STT";
/// 0 = low, 1 = normal, 2 = critical.
const URGENCY_NORMAL: u8 = 1;
/// Let the notification server pick the timeout.
const EXPIRE_DEFAULT: i32 = -1;

/// Sends failure notices to the session's notification server.
///
/// `pub` (not `pub(crate)`) because it is held in a `pub` field on the `pub`
/// `SuperSTTDaemon` — the same reason `keyboard::Simulator` is public.
pub struct Notifier {
    inner: Inner,
    /// Id of the last notification sent, passed as `replaces_id` so repeated
    /// failures replace the previous bubble instead of stacking. The spec
    /// treats 0 as "do not replace".
    last_id: u32,
}

enum Inner {
    /// Session-bus connection, established on first send and cached.
    Dbus(Option<Connection>),
    #[cfg(test)]
    Fake {
        fail: bool,
        sent: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    },
}

impl Notifier {
    #[must_use]
    pub fn dbus() -> Self {
        Self {
            inner: Inner::Dbus(None),
            last_id: 0,
        }
    }

    /// Deliver `body` as a desktop notification.
    ///
    /// # Errors
    /// Returns an error when no session bus is reachable, or when no
    /// notification server owns `org.freedesktop.Notifications`. Callers treat
    /// both the same way: there is nowhere to show a notification.
    ///
    /// # Panics
    /// Never in practice: the `expect` below only unwraps the connection slot
    /// this same call just populated a few lines above.
    pub async fn send(&mut self, body: &str) -> Result<()> {
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
                            SUMMARY,
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
                sent.lock().unwrap().push(body.to_string());
                Ok(())
            }
        }
    }

    /// A notifier that records what it was asked to send, or fails every send
    /// when `fail` is true.
    #[cfg(test)]
    pub(crate) fn fake(fail: bool) -> (Self, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
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
/// Typing needs a keyboard channel, so `typed` is attempted only for write-mode
/// recordings and otherwise degrades to a log line. `auto` degrades the same way
/// once notification delivery has failed.
pub(crate) async fn deliver(
    method: NotificationMethod,
    notifier: &mut Notifier,
    typer: &mut Typer,
    notice: &'static str,
    write_mode: bool,
) {
    match method {
        NotificationMethod::Off => {
            info!("Recording failure: {notice} (surfacing disabled)");
        }
        NotificationMethod::Typed => type_or_log(typer, notice, write_mode).await,
        NotificationMethod::Dbus => {
            if let Err(e) = notifier.send(notice).await {
                warn!("Could not deliver failure notification ({notice}): {e}");
            }
        }
        NotificationMethod::Auto => {
            if let Err(e) = notifier.send(notice).await {
                warn!("Notification delivery failed ({e}); falling back to typing");
                type_or_log(typer, notice, write_mode).await;
            }
        }
    }
}

async fn type_or_log(typer: &mut Typer, notice: &'static str, write_mode: bool) {
    if write_mode {
        typer.type_notice(notice).await;
    } else {
        info!("Recording failure: {notice} (not in write mode, nothing typed)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::keyboard::Simulator;
    use crate::output::notice;

    /// Build a typer whose keystrokes land in a buffer we can assert on.
    fn typer() -> (Typer, std::sync::Arc<std::sync::Mutex<String>>) {
        let (sim, buf) = Simulator::capture();
        (Typer::new(sim), buf)
    }

    #[tokio::test(start_paused = true)]
    async fn off_types_nothing_and_sends_nothing() {
        let (mut n, sent) = Notifier::fake(false);
        let (mut t, typed) = typer();

        deliver(
            NotificationMethod::Off,
            &mut n,
            &mut t,
            notice::TRANSCRIPTION_FAILED,
            true,
        )
        .await;

        assert!(sent.lock().unwrap().is_empty());
        assert_eq!(*typed.lock().unwrap(), "");
    }

    #[tokio::test(start_paused = true)]
    async fn typed_types_the_notice_and_sends_nothing() {
        let (mut n, sent) = Notifier::fake(false);
        let (mut t, typed) = typer();

        deliver(
            NotificationMethod::Typed,
            &mut n,
            &mut t,
            notice::TRANSCRIPTION_FAILED,
            true,
        )
        .await;

        assert!(sent.lock().unwrap().is_empty());
        assert_eq!(*typed.lock().unwrap(), notice::TRANSCRIPTION_FAILED);
    }

    #[tokio::test(start_paused = true)]
    async fn dbus_sends_the_notice_and_types_nothing() {
        let (mut n, sent) = Notifier::fake(false);
        let (mut t, typed) = typer();

        deliver(
            NotificationMethod::Dbus,
            &mut n,
            &mut t,
            notice::RECORDING_FAILED,
            true,
        )
        .await;

        assert_eq!(*sent.lock().unwrap(), vec![notice::RECORDING_FAILED]);
        assert_eq!(*typed.lock().unwrap(), "");
    }

    /// `dbus` is the deliberate "notification or nothing" choice — a failed
    /// delivery must NOT fall back to typing.
    #[tokio::test(start_paused = true)]
    async fn dbus_does_not_type_when_delivery_fails() {
        let (mut n, _sent) = Notifier::fake(true);
        let (mut t, typed) = typer();

        deliver(
            NotificationMethod::Dbus,
            &mut n,
            &mut t,
            notice::RECORDING_FAILED,
            true,
        )
        .await;

        assert_eq!(*typed.lock().unwrap(), "");
    }

    #[tokio::test(start_paused = true)]
    async fn auto_sends_and_does_not_type_when_delivery_succeeds() {
        let (mut n, sent) = Notifier::fake(false);
        let (mut t, typed) = typer();

        deliver(
            NotificationMethod::Auto,
            &mut n,
            &mut t,
            notice::NO_MODEL_LOADED,
            true,
        )
        .await;

        assert_eq!(*sent.lock().unwrap(), vec![notice::NO_MODEL_LOADED]);
        assert_eq!(*typed.lock().unwrap(), "");
    }

    /// The fallback that keeps a bare compositor with no notification server
    /// from silently swallowing failures.
    #[tokio::test(start_paused = true)]
    async fn auto_falls_back_to_typing_when_delivery_fails() {
        let (mut n, _sent) = Notifier::fake(true);
        let (mut t, typed) = typer();

        deliver(
            NotificationMethod::Auto,
            &mut n,
            &mut t,
            notice::NO_MODEL_LOADED,
            true,
        )
        .await;

        assert_eq!(*typed.lock().unwrap(), notice::NO_MODEL_LOADED);
    }

    /// Without write mode there is no keyboard channel, so `typed` logs.
    #[tokio::test(start_paused = true)]
    async fn typed_without_write_mode_types_nothing() {
        let (mut n, _sent) = Notifier::fake(false);
        let (mut t, typed) = typer();

        deliver(
            NotificationMethod::Typed,
            &mut n,
            &mut t,
            notice::TRANSCRIPTION_FAILED,
            false,
        )
        .await;

        assert_eq!(*typed.lock().unwrap(), "");
    }

    /// Notifications are mode-independent: a non-write recording still notifies.
    #[tokio::test(start_paused = true)]
    async fn auto_still_notifies_without_write_mode() {
        let (mut n, sent) = Notifier::fake(false);
        let (mut t, typed) = typer();

        deliver(
            NotificationMethod::Auto,
            &mut n,
            &mut t,
            notice::TRANSCRIPTION_FAILED,
            false,
        )
        .await;

        assert_eq!(*sent.lock().unwrap(), vec![notice::TRANSCRIPTION_FAILED]);
        assert_eq!(*typed.lock().unwrap(), "");
    }

    /// And falls through to nothing when it cannot notify and cannot type.
    #[tokio::test(start_paused = true)]
    async fn auto_without_write_mode_and_failed_delivery_types_nothing() {
        let (mut n, _sent) = Notifier::fake(true);
        let (mut t, typed) = typer();

        deliver(
            NotificationMethod::Auto,
            &mut n,
            &mut t,
            notice::TRANSCRIPTION_FAILED,
            false,
        )
        .await;

        assert_eq!(*typed.lock().unwrap(), "");
    }
}
