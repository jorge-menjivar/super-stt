// SPDX-License-Identifier: GPL-3.0-only
//! Desktop notifications for synthesis failures.
//!
//! They go through the daemon's one desktop notifier, the one speech to text's
//! failures use: freedesktop notifications on Linux, Notification Center on
//! macOS. See [`crate::output::notification`].
//!
//! What a bubble says is decided in [`crate::tts::output::notice`], including how
//! backend-authored text is made safe to put in a body; this module only picks
//! whether to send it.

use crate::tts::output::notice::Failure;
use log::{info, warn};
use super_voice_shared::tts::models::notification_method::NotificationMethod;

pub use crate::output::notification::Notifier;

/// What the daemon's one notification setting means for a speech failure.
/// Speech to text's setting has two more choices than text to speech ever
/// had: `desktop`, which is what `auto` already does here, and `typed`, which
/// types the failure into the focused field — meaningless for an utterance
/// nobody is typing. Both read as `auto`; `off` stays off.
#[must_use]
pub(crate) fn method_for_speech(
    setting: super_voice_shared::models::notification_method::NotificationMethod,
) -> NotificationMethod {
    use super_voice_shared::models::notification_method::NotificationMethod as Setting;
    match setting {
        Setting::Off => NotificationMethod::Off,
        Setting::Auto | Setting::Desktop | Setting::Typed => NotificationMethod::Auto,
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
    use crate::output::notification::APP_NAME;
    use crate::tts::output::notice::Origin;

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
