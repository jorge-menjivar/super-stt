// SPDX-License-Identifier: GPL-3.0-only
//! Desktop notification delivery for recording failures.
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

/// Sent as the notification's `app_name`, which is where the user learns who
/// this bubble is from. The summary is free to name the failure instead.
const APP_NAME: &str = "Super STT";
/// Installed into `share/icons/hicolor/scalable/apps` by the justfile.
const APP_ICON: &str = "super-stt-app";
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
/// Typing needs a keyboard channel, so `typed` is attempted only for write-mode
/// recordings and otherwise degrades to a log line. `auto` degrades the same way
/// once notification delivery has failed.
pub(crate) async fn deliver(
    method: NotificationMethod,
    notifier: &mut Notifier,
    typer: &mut Typer,
    failure: &Failure,
    write_mode: bool,
) {
    match method {
        NotificationMethod::Off => {
            info!(
                "Recording failure: {} — {} (surfacing disabled)",
                failure.summary, failure.body
            );
        }
        NotificationMethod::Typed => type_or_log(typer, failure.typed, write_mode).await,
        NotificationMethod::Dbus => {
            if let Err(e) = notifier.send(failure.summary, &failure.body).await {
                warn!(
                    "Could not deliver failure notification ({}): {e}",
                    failure.summary
                );
            }
        }
        NotificationMethod::Auto => {
            if let Err(e) = notifier.send(failure.summary, &failure.body).await {
                warn!("Notification delivery failed ({e}); falling back to typing");
                type_or_log(typer, failure.typed, write_mode).await;
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
    use crate::output::notice::{self, Origin};

    /// Build a typer whose keystrokes land in a buffer we can assert on.
    fn typer() -> (Typer, std::sync::Arc<std::sync::Mutex<String>>) {
        let (sim, buf) = Simulator::capture();
        (Typer::new(sim), buf)
    }

    /// A transcription failure carrying a backend's reason — the shape the user
    /// hits most often, and the one that has both a summary and a body to check.
    fn backend_failure() -> Failure {
        Failure::transcription_failed(Origin::Backend, "Could not reach the server (write_failed)")
    }

    #[tokio::test(start_paused = true)]
    async fn off_types_nothing_and_sends_nothing() {
        let (mut n, sent) = Notifier::fake(false);
        let (mut t, typed) = typer();

        deliver(
            NotificationMethod::Off,
            &mut n,
            &mut t,
            &backend_failure(),
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
            &backend_failure(),
            true,
        )
        .await;

        assert!(sent.lock().unwrap().is_empty());
        assert_eq!(*typed.lock().unwrap(), notice::TRANSCRIPTION_FAILED);
    }

    /// The rule the notification channel relaxed and this one did not: what goes
    /// into the user's focused window is the fixed marker, never the backend's
    /// reason, however much of it the bubble would have shown.
    #[tokio::test(start_paused = true)]
    async fn typing_never_carries_the_reason() {
        let (mut n, _sent) = Notifier::fake(true);
        let (mut t, typed) = typer();

        deliver(
            NotificationMethod::Auto,
            &mut n,
            &mut t,
            &backend_failure(),
            true,
        )
        .await;

        let typed = typed.lock().unwrap().clone();
        assert_eq!(typed, notice::TRANSCRIPTION_FAILED);
        assert!(
            !typed.contains("write_failed"),
            "backend text was typed into the focused window: {typed}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dbus_sends_the_summary_and_body_and_types_nothing() {
        let (mut n, sent) = Notifier::fake(false);
        let (mut t, typed) = typer();

        deliver(
            NotificationMethod::Dbus,
            &mut n,
            &mut t,
            &Failure::recording_failed("Audio device disappeared mid-take"),
            true,
        )
        .await;

        assert_eq!(
            *sent.lock().unwrap(),
            vec![(
                "Recording failed".to_string(),
                "Audio device disappeared mid-take".to_string()
            )]
        );
        assert_eq!(*typed.lock().unwrap(), "");
    }

    /// The bug this replaced: a bubble that named the app twice and the reason
    /// not at all.
    #[tokio::test(start_paused = true)]
    async fn the_bubble_carries_the_reason_and_not_a_second_app_name() {
        let (mut n, sent) = Notifier::fake(false);
        let (mut t, _typed) = typer();

        deliver(
            NotificationMethod::Dbus,
            &mut n,
            &mut t,
            &backend_failure(),
            false,
        )
        .await;

        let sent = sent.lock().unwrap().clone();
        let (summary, body) = sent.first().expect("one notification");
        assert_eq!(summary, "Transcription failed");
        assert_eq!(
            body,
            "Backend error: Could not reach the server (write_failed)"
        );
        assert!(
            !summary.contains(APP_NAME) && !body.contains(APP_NAME),
            "the app name is the notification's own field, not text"
        );
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
            &Failure::recording_failed("d"),
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
            &Failure::no_model_loaded(),
            true,
        )
        .await;

        assert_eq!(
            *sent.lock().unwrap(),
            vec![(
                "No model loaded".to_string(),
                "Load a model and try again.".to_string()
            )]
        );
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
            &Failure::no_model_loaded(),
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
            &backend_failure(),
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
            &backend_failure(),
            false,
        )
        .await;

        assert_eq!(sent.lock().unwrap().len(), 1);
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
            &backend_failure(),
            false,
        )
        .await;

        assert_eq!(*typed.lock().unwrap(), "");
    }
}
