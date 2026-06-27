// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::client::internal::session::SETTINGS_SCOPES;
use crate::ui::messages::Message;
use futures_util::SinkExt;
use log::{info, warn};

use super::events::settings_widget_event_to_message;

/// Wrapper for `Subscription::run_with` so the subscription restarts when the counter changes.
#[derive(Hash)]
pub(super) struct UdpSubscriptionId(pub(super) u64);

/// Subscribe to the daemon's `/events` SSE stream for the settings UI's
/// audio meter and recording-state indicator. Reuses the token already
/// cached for normal config calls — [`SETTINGS_SCOPES`] grants the
/// recording / visualization / daemon-status topics below.
pub(super) const SETTINGS_APP_ID: super_stt_shared::daemon::session::AppId =
    super_stt_shared::daemon::session::AppId("super-stt-app");
const SETTINGS_APP_NAME: &str = "Super STT Settings App";
/// Topics the settings app subscribes to over `GET /events`.
///
/// `recording_state` drives the recording badge (`recording_events`),
/// `frequency_bands` drives the audio meter (`audio_visualization`), and
/// `daemon_status_changed` / `download_progress` / `registry_install`
/// drive the model-switch progress bar and Download-tab install cards
/// (`daemon_status`).
const SETTINGS_TOPICS: &[&str] = &[
    "recording_state",
    "frequency_bands",
    "daemon_status_changed",
    "download_progress",
    "registry_install",
];

/// Self-healing `/events` subscription for the settings UI's audio
/// meter + recording-status badge. Routes through the shared
/// [`run_widget_subscription`] helper so silent drops, idle wedges,
/// and daemon-side revocations all auto-recover with backoff.
pub(super) fn audio_events_subscription(
    _id: &UdpSubscriptionId,
) -> std::pin::Pin<Box<dyn cosmic::iced::futures::Stream<Item = Message> + Send>> {
    use futures_util::StreamExt;
    use super_stt_shared::daemon::widget_subscription::{
        WidgetSubscriptionConfig, WidgetSubscriptionUpdate, run_widget_subscription,
    };
    use super_stt_shared::validation::get_http_socket_path;

    Box::pin(cosmic::iced::stream::channel(100, async |mut channel| {
        let config = WidgetSubscriptionConfig::new(
            SETTINGS_APP_ID,
            SETTINGS_APP_NAME,
            SETTINGS_SCOPES,
            SETTINGS_TOPICS,
        );
        let mut updates = Box::pin(run_widget_subscription(get_http_socket_path(), config));
        info!("Settings subscription starting");
        while let Some(update) = updates.next().await {
            let msg = match update {
                WidgetSubscriptionUpdate::Connected => {
                    // A successful (re)subscribe of the live event stream.
                    // Clears any sticky `Blocked` / `Error` state on the
                    // daemon-status badge (without this the settings UI sits in
                    // whatever state it was last in, e.g. Blocked from a denial
                    // the user has since cleared) AND triggers a current-model
                    // re-fetch now that live events are flowing — so a model
                    // that finished loading before this subscription completed
                    // is still picked up.
                    Message::EventStreamConnected
                }
                WidgetSubscriptionUpdate::Event(evt) => {
                    match settings_widget_event_to_message(&evt) {
                        Some(m) => m,
                        None => continue,
                    }
                }
                WidgetSubscriptionUpdate::Disconnected { reason } => {
                    warn!("Settings /events disconnected ({reason}); reconnecting");
                    continue;
                }
                WidgetSubscriptionUpdate::NeedsReauth { reason } => {
                    warn!(
                        "Settings session needs re-auth ({reason}); will re-consent on next attempt"
                    );
                    continue;
                }
                WidgetSubscriptionUpdate::Blocked { reason } => {
                    warn!("Settings session blocked by user denial ({reason}); subscription ended");
                    // Forward to the update loop so the UI flips to
                    // the Blocked state (Retry button) instead of
                    // sitting silently with a dead audio meter.
                    Message::WidgetBlocked(reason)
                }
            };
            if channel.send(msg).await.is_err() {
                break;
            }
        }
        info!("Settings subscription ended");
    }))
}

#[cfg(test)]
mod tests {
    use super::{SETTINGS_SCOPES, SETTINGS_TOPICS};
    use super_stt_shared::daemon::widget_subscription::uncovered_topic;

    #[test]
    fn settings_scopes_cover_subscribed_topics() {
        // Guard the hand-maintained SETTINGS_SCOPES / SETTINGS_TOPICS lists:
        // every subscribed topic must be granted by a requested scope, or the
        // daemon refuses the whole stream with `403 scope_denied`. The mapping
        // lives in super-stt-shared and is pinned to the daemon's
        // `Topic::required_scope`.
        assert_eq!(uncovered_topic(SETTINGS_SCOPES, SETTINGS_TOPICS), None);
    }
}
