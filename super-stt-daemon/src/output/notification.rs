// SPDX-License-Identifier: GPL-3.0-only
//! Desktop notification delivery for recording failures.
//!
//! On Linux this is the freedesktop Desktop Notifications interface
//! (`org.freedesktop.Notifications`), which every mainstream desktop provides —
//! GNOME, KDE Plasma, COSMIC, XFCE, MATE, Cinnamon, `LXQt` — as do the
//! standalone servers used on bare compositors (mako, dunst, swaync). One code
//! path covers all of them; nothing there is desktop-specific.
//!
//! On macOS it is Notification Center: through `UNUserNotificationCenter` when
//! the daemon runs from inside `Super STT.app`, and through `osascript` when it
//! is a bare binary. See [`Inner`] for what each costs relative to the Linux
//! path.
//!
//! What a bubble says is decided in [`crate::output::notice`], including how
//! backend-authored text is made safe to put in a body; this module only carries
//! it to the notification service.

use crate::output::notice::Failure;
use crate::output::typer::Typer;
use anyhow::{Context, Result};
#[cfg(target_os = "linux")]
use futures::StreamExt;
use log::{debug, info, warn};
#[cfg(target_os = "linux")]
use std::collections::HashMap;
use super_stt_shared::models::notification_method::NotificationMethod;
#[cfg(target_os = "linux")]
use zbus::Connection;
#[cfg(target_os = "linux")]
use zbus::zvariant::Value;

#[cfg(target_os = "linux")]
const NOTIFY_BUS: &str = "org.freedesktop.Notifications";
#[cfg(target_os = "linux")]
const NOTIFY_PATH: &str = "/org/freedesktop/Notifications";
#[cfg(target_os = "linux")]
const NOTIFY_IFACE: &str = "org.freedesktop.Notifications";

/// Sent as the notification's `app_name`, which is where the user learns who
/// this bubble is from. The summary is free to name the failure instead.
const APP_NAME: &str = "Super STT";
/// Installed into `share/icons/hicolor/scalable/apps` by the justfile.
#[cfg(target_os = "linux")]
const APP_ICON: &str = "super-stt-app";
/// 0 = low, 1 = normal, 2 = critical.
#[cfg(target_os = "linux")]
const URGENCY_NORMAL: u8 = 1;
/// Let the notification server pick the timeout.
#[cfg(target_os = "linux")]
const EXPIRE_DEFAULT: i32 = -1;

/// The action key offered on update-available bubbles. The spec's
/// `default` action is what most servers bind to clicking the bubble
/// itself, so it is the one that makes "click the notification to open
/// the app" work everywhere.
pub(crate) const OPEN_APP_ACTION_KEY: &str = "default";

/// Sends failure notices to the session's notification server.
///
/// `pub` (not `pub(crate)`) because it is held in a `pub` field on the `pub`
/// `SuperSTTDaemon` — the same reason `keyboard::Simulator` is public.
pub struct Notifier {
    inner: Inner,
    /// Id of the last notification sent, passed as `replaces_id` so repeated
    /// failures replace the previous bubble instead of stacking. The spec
    /// treats 0 as "do not replace".
    ///
    /// Linux only. Notification Center replaces by request identifier
    /// instead, and `notification_center` posts every banner under one.
    #[cfg(target_os = "linux")]
    last_id: u32,
}

/// Every `(summary, body)` a [`Notifier::fake`] was asked to send.
#[cfg(test)]
type Sent = std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>;

/// Where a notification actually goes.
enum Inner {
    /// Session-bus connection, established on first send and cached.
    #[cfg(target_os = "linux")]
    Dbus(Option<Connection>),
    /// macOS Notification Center, for a daemon inside `Super STT.app`, posted
    /// by the app's executable at this path; see `notification_center`. The
    /// banner carries the app's name and icon, and a post the user has not
    /// allowed is reported as a failure.
    ///
    /// No buttons: clicking the banner opens the app, which is all the
    /// "Open Super STT" action on an update bubble asks for, so
    /// [`Notifier::send_with_actions`] drops the actions it is given.
    #[cfg(target_os = "macos")]
    NotificationCenter(std::path::PathBuf),
    /// macOS Notification Center, via `osascript`, for a bare daemon binary,
    /// which has no app to post through.
    ///
    /// Stateless — there is no connection to cache, because each notification
    /// is a fresh short-lived process. Three things are genuinely weaker here
    /// than on the other two paths, and callers should know which:
    ///
    /// - **Not Super STT's banner.** macOS titles it with the posting
    ///   process, which reads "Script Editor"; see [`NOTIFY_APPLESCRIPT`].
    /// - **No actions.** `display notification` posts a banner and nothing
    ///   else, and a click opens Script Editor. The "Open Super STT" action on
    ///   an update bubble is simply absent, so
    ///   [`Notifier::send_with_actions`] drops what it is given rather than
    ///   pretending a click can arrive.
    /// - **No delivery confirmation.** `osascript` exits 0 once it has handed
    ///   the banner to Notification Center. If the user has notifications
    ///   turned off for the posting application, or Do Not Disturb is on, the
    ///   banner is dropped silently and this still reports success — so
    ///   [`NotificationMethod::Auto`] will not fall back to typing in that
    ///   case, because nothing told it to.
    #[cfg(target_os = "macos")]
    AppleScript,
    #[cfg(test)]
    Fake { fail: bool, sent: Sent },
}

impl Notifier {
    /// A notifier that posts to this platform's desktop notification service.
    #[must_use]
    pub fn desktop() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            inner: Inner::Dbus(None),
            #[cfg(target_os = "macos")]
            inner: crate::output::notification_center::notifier()
                .map_or(Inner::AppleScript, Inner::NotificationCenter),
            #[cfg(target_os = "linux")]
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
        self.send_with_actions(summary, body, &[]).await.map(|_| ())
    }

    /// Deliver `summary` and `body` as a desktop notification carrying
    /// `actions` (pairs of action key and label), returning the notification
    /// id the server assigned. When the user picks one, the notification
    /// server emits `ActionInvoked` on the same bus; the caller is
    /// responsible for listening for it (see
    /// [`crate::daemon::self_update_handlers`]).
    ///
    /// # Errors
    /// As [`Self::send`].
    ///
    /// # Panics
    /// As [`Self::send`].
    pub async fn send_with_actions(
        &mut self,
        summary: &str,
        body: &str,
        actions: &[(&str, &str)],
    ) -> Result<u32> {
        match &mut self.inner {
            #[cfg(target_os = "linux")]
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
                let actions: Vec<&str> = actions
                    .iter()
                    .flat_map(|(key, label)| [*key, *label])
                    .collect();

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
                Ok(id)
            }
            #[cfg(target_os = "macos")]
            inner @ (Inner::NotificationCenter(_) | Inner::AppleScript) => {
                if !actions.is_empty() {
                    debug!(
                        "dropping {} notification action(s): macOS banners carry none",
                        actions.len()
                    );
                }
                if let Inner::NotificationCenter(notifier) = inner {
                    crate::output::notification_center::post(notifier, summary, body).await?;
                } else {
                    post_with_osascript(summary, body).await?;
                }
                // Nothing on this platform consumes an id: there is no
                // `replaces_id` to feed and no action click to correlate.
                // Reporting 0 — the freedesktop "no replacement" value — keeps
                // the signature honest rather than inventing a handle that
                // addresses nothing.
                debug!("Delivered failure notification to Notification Center");
                Ok(0)
            }
            #[cfg(test)]
            Inner::Fake { fail, sent } => {
                if *fail {
                    anyhow::bail!("fake notifier: delivery failed");
                }
                sent.lock()
                    .unwrap()
                    .push((summary.to_string(), body.to_string()));
                Ok(0)
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
                #[cfg(target_os = "linux")]
                last_id: 0,
            },
            sent,
        )
    }

    /// Resolve when the user activates an action on the notification `id`,
    /// returning the action key (e.g. [`OPEN_APP_ACTION_KEY`]). The
    /// `ActionInvoked` signal carries the id of the bubble the action came
    /// from, so only that bubble resolves the wait — another app's
    /// notifications are ignored.
    ///
    /// The subscription is built lazily per call on `conn`; a failure to
    /// subscribe resolves `None` (the click simply does nothing, which is how
    /// the bubble behaved before actions existed).
    ///
    /// # Errors
    /// Never: every failure path resolves `None` rather than erroring, since
    /// a missed click is not worth surfacing.
    #[cfg(target_os = "linux")]
    pub async fn wait_for_action(conn: &Connection, id: u32) -> Option<String> {
        let proxy = zbus::Proxy::new(conn, NOTIFY_BUS, NOTIFY_PATH, NOTIFY_IFACE)
            .await
            .ok()?;
        let mut signals = proxy.receive_signal("ActionInvoked").await.ok()?;
        let signal = signals.next().await?;
        let (signal_id, key): (u32, String) = signal.body().deserialize().ok()?;
        (signal_id == id).then_some(key)
    }

    /// A clone of the cached session-bus connection, if one has been
    /// established by a prior send. Used by callers that must wait for an
    /// `ActionInvoked` without holding the notifier's mutex across the wait.
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn connection(&self) -> Option<Connection> {
        match &self.inner {
            Inner::Dbus(slot) => slot.clone(),
            #[cfg(test)]
            Inner::Fake { .. } => None,
        }
    }
}

/// The `AppleScript` that posts one banner.
///
/// Same argv discipline as the consent dialog: the script builds no strings,
/// so a notification body cannot become `AppleScript`. That matters more here
/// than it looks — a failure body can carry text a *backend* wrote
/// (`crate::output::notice` makes it safe to display, which is a different
/// question from safe to evaluate).
///
/// The three arguments are [`APP_NAME`], the summary, and the body, in that
/// order. Note that the summary lands in the *subtitle*: a macOS banner
/// titles itself with the posting process, which here is `osascript` and
/// reads "Script Editor", so the product name has to occupy the title for the
/// banner to be identifiable at all. On the freedesktop side that name is a
/// field of its own (`app_name`) and the summary is the title, which is why
/// the two platforms lay the same three strings out differently.
#[cfg(target_os = "macos")]
const NOTIFY_APPLESCRIPT: &str = r"on run argv
	display notification (item 3 of argv) with title (item 1 of argv) subtitle (item 2 of argv)
end run
";

/// Absolute path to the system `AppleScript` interpreter — see the consent
/// module for why this is never resolved through `PATH`.
#[cfg(target_os = "macos")]
const OSASCRIPT: &str = "/usr/bin/osascript";

/// How long to wait for `osascript` to hand the banner over before giving up.
///
/// Posting is a fast local call; a wait this long past it means something is
/// wrong with the process rather than slow. Bounded at all because this is
/// awaited on the recording path, and a notification that cannot be posted
/// must not hold up telling the user by other means.
#[cfg(target_os = "macos")]
const NOTIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Post `summary`/`body` to Notification Center.
///
/// # Errors
/// When `osascript` cannot be spawned, exits non-zero, or outlives
/// [`NOTIFY_TIMEOUT`]. A banner the user has muted is *not* an error — see
/// [`Inner::AppleScript`] for what this can and cannot detect.
#[cfg(target_os = "macos")]
async fn post_with_osascript(summary: &str, body: &str) -> Result<()> {
    use tokio::io::AsyncWriteExt as _;

    let mut child = tokio::process::Command::new(OSASCRIPT)
        // `-` reads the script from stdin; the rest becomes `argv`.
        .arg("-")
        .arg(APP_NAME)
        .arg(summary)
        .arg(body)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("could not run osascript to post a notification")?;

    let mut stdin = child
        .stdin
        .take()
        .context("osascript child had no stdin pipe")?;
    stdin
        .write_all(NOTIFY_APPLESCRIPT.as_bytes())
        .await
        .context("could not hand the notification script to osascript")?;
    // `osascript -` runs nothing until stdin reaches EOF.
    drop(stdin);

    let output = match tokio::time::timeout(NOTIFY_TIMEOUT, child.wait_with_output()).await {
        Ok(result) => result.context("osascript failed while posting a notification")?,
        Err(_) => {
            anyhow::bail!("osascript did not post the notification within {NOTIFY_TIMEOUT:?}")
        }
    };
    anyhow::ensure!(
        output.status.success(),
        "osascript exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim(),
    );
    Ok(())
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
        NotificationMethod::Desktop => {
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

/// Route a failure that happened because the keyboard itself could not be set
/// up.
///
/// [`deliver`] cannot carry this one: its typed channel needs the very keyboard
/// that failed. So every method except `off` sends a notification — `typed`
/// included, because typing is exactly what cannot happen, and a user who chose
/// it still needs to learn why nothing appeared. Before this existed the
/// failure was logged and nothing else, so a recording started from a global
/// shortcut failed with no sign at all.
pub(crate) async fn deliver_without_keyboard(
    method: NotificationMethod,
    notifier: &mut Notifier,
    failure: &Failure,
) {
    if matches!(method, NotificationMethod::Off) {
        info!(
            "Recording failure: {} — {} (surfacing disabled)",
            failure.summary, failure.body
        );
        return;
    }
    if let Err(e) = notifier.send(failure.summary, &failure.body).await {
        warn!(
            "Could not deliver failure notification ({}): {e}",
            failure.summary
        );
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
            NotificationMethod::Desktop,
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
            NotificationMethod::Desktop,
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

    /// With no keyboard there is no typed channel, so even `typed` notifies —
    /// otherwise a user who chose it would learn nothing at all.
    #[tokio::test(start_paused = true)]
    async fn a_keyboard_failure_notifies_under_every_method_but_off() {
        for method in [
            NotificationMethod::Auto,
            NotificationMethod::Desktop,
            NotificationMethod::Typed,
        ] {
            let (mut n, sent) = Notifier::fake(false);
            deliver_without_keyboard(
                method,
                &mut n,
                &Failure::keyboard_unavailable("Add the daemon under Accessibility"),
            )
            .await;
            assert_eq!(
                *sent.lock().unwrap(),
                vec![(
                    "Cannot type".to_string(),
                    "Add the daemon under Accessibility".to_string()
                )],
                "{method:?}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_keyboard_failure_stays_silent_when_surfacing_is_off() {
        let (mut n, sent) = Notifier::fake(false);
        deliver_without_keyboard(
            NotificationMethod::Off,
            &mut n,
            &Failure::keyboard_unavailable("d"),
        )
        .await;
        assert!(sent.lock().unwrap().is_empty());
    }

    /// `dbus` is the deliberate "notification or nothing" choice — a failed
    /// delivery must NOT fall back to typing.
    #[tokio::test(start_paused = true)]
    async fn dbus_does_not_type_when_delivery_fails() {
        let (mut n, _sent) = Notifier::fake(true);
        let (mut t, typed) = typer();

        deliver(
            NotificationMethod::Desktop,
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
