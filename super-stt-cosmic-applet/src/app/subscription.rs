// SPDX-License-Identifier: GPL-3.0-only
use cosmic::iced::futures::Stream;
use futures_util::{SinkExt, StreamExt};
use log::{info, warn};
use std::pin::Pin;

use crate::app::Message;
use crate::daemon::identity;
use crate::util::f64_to_f32;
use super_stt_shared::daemon::http_client::WidgetEvent;
use super_stt_shared::daemon::widget_subscription::{
    WidgetSubscriptionConfig, WidgetSubscriptionUpdate, run_widget_subscription,
};
use super_stt_shared::validation::get_http_socket_path;

/// Interval at which the applet pings the daemon to check its health.
pub(super) const PING_INTERVAL_SECS: u64 = 5;

/// Wrapper for `Subscription::run_with` so the subscription restarts when the counter changes.
#[derive(Hash)]
pub(super) struct UdpSubscriptionId(pub(super) u64);

/// Subscribes to the daemon's `GET /events` SSE stream and forwards
/// each event as a typed [`Message`]. The subscription is self-healing
/// — if the SSE stream drops, the daemon revokes the session, or the
/// connection wedges past the keepalive deadline, the shared
/// [`run_widget_subscription`] helper reconnects (with backoff) and
/// re-auths automatically. The wrapping iced subscription only
/// terminates when the applet is shutting down.
pub(super) fn applet_events_subscription(
    _id: &UdpSubscriptionId,
) -> Pin<Box<dyn Stream<Item = Message> + Send>> {
    Box::pin(cosmic::iced::stream::channel(100, async |mut channel| {
        let config = WidgetSubscriptionConfig::new(
            identity::APP_ID,
            identity::APP_NAME,
            identity::SCOPES,
            identity::TOPICS,
        );
        let mut updates = Box::pin(run_widget_subscription(get_http_socket_path(), config));
        info!("Widget subscription starting");
        while let Some(update) = updates.next().await {
            let msg = applet_subscription_update_to_message(update);
            if channel.send(msg).await.is_err() {
                break; // applet shutting down
            }
        }
        info!("Widget subscription ended");
    }))
}

/// Project a [`WidgetSubscriptionUpdate`] from the shared helper into
/// the applet's typed [`Message`] enum.
fn applet_subscription_update_to_message(update: WidgetSubscriptionUpdate) -> Message {
    match update {
        // Route a successful (re)connect into the existing
        // `DaemonConnected` handler so it clears any prior
        // `Error("revoked: …")` state and resets retry. Without this,
        // a user-denied → daemon-restart → auto-reconnect cycle would
        // leave the UI stuck on the stale "revoked" error even though
        // the subscription is live again.
        WidgetSubscriptionUpdate::Connected => Message::DaemonConnected,
        WidgetSubscriptionUpdate::Event(evt) => widget_event_to_message(evt),
        WidgetSubscriptionUpdate::Disconnected { reason } => {
            warn!("Widget /events disconnected ({reason}); reconnecting");
            Message::WidgetSubscriptionError(reason)
        }
        WidgetSubscriptionUpdate::NeedsReauth { reason } => {
            warn!("Widget session needs re-auth ({reason}); will re-consent on next attempt");
            Message::WidgetRevoked(reason)
        }
        WidgetSubscriptionUpdate::Blocked { reason } => {
            warn!("Widget subscription blocked ({reason}); stream terminated");
            Message::WidgetBlocked(reason)
        }
    }
}

/// Decode a base64-encoded little-endian `f32` buffer carried by an SSE
/// event payload. Returns an empty vector on a missing or malformed value.
fn b64_to_f32_vec(s: Option<&str>) -> Vec<f32> {
    let Some(s) = s else { return Vec::new() };
    let Ok(bytes) = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, s) else {
        return Vec::new();
    };
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Translate one [`WidgetEvent`] into the applet's typed [`Message`]
/// enum. Unknown event names are surfaced as `WidgetOtherEvent(name)`
/// so the update loop can log them at most once per event type.
fn widget_event_to_message(evt: WidgetEvent) -> Message {
    use serde_json::Value;

    let p: &Value = &evt.payload;
    match evt.name.as_str() {
        "recording_state" => Message::WidgetRecordingState(
            p.get("is_recording")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        ),
        "frequency_bands" => Message::WidgetFrequencyBands {
            bands: b64_to_f32_vec(p.get("bands_b64").and_then(Value::as_str)),
            sample_rate: f64_to_f32(p.get("sample_rate").and_then(Value::as_f64).unwrap_or(0.0)),
            total_energy: f64_to_f32(p.get("total_energy").and_then(Value::as_f64).unwrap_or(0.0)),
        },
        "revoked" => Message::WidgetRevoked(
            p.get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
        ),
        "transcribing_started" => Message::WidgetTranscribingStarted,
        "transcribing_stopped" => Message::WidgetTranscribingStopped,
        "subscribed" | "error" => Message::WidgetOtherEvent(evt.name),
        other => Message::WidgetOtherEvent(other.to_string()),
    }
}

#[cfg(test)]
mod widget_subscription_mapping_tests {
    //! These tests pin the mapping between the shared
    //! `WidgetSubscriptionUpdate` variants and the applet's `Message`
    //! enum. Each variant has a load-bearing UX contract:
    //!
    //! - `Connected` → `Message::DaemonConnected` so the existing
    //!   handler clears any prior `Error("revoked: …")` state after
    //!   auto-recovery. If this mapping silently changes, the applet
    //!   gets stuck on a stale revoked banner forever.
    //! - `Blocked` → `Message::WidgetBlocked` so the UI flips to the
    //!   sticky "Authorization denied" view with a Retry button. If
    //!   this maps to anything else, the helper's
    //!   stop-spamming-on-deny fix becomes invisible.
    //! - `NeedsReauth` → `Message::WidgetRevoked` so the UI shows a
    //!   transient revoked banner while the helper does the
    //!   `session::forget` → fresh-consent cycle.
    //! - `Disconnected` → `Message::WidgetSubscriptionError` so the
    //!   UI doesn't change state during the helper's internal
    //!   backoff/reconnect — the helper auto-recovers.
    use super::*;

    #[test]
    fn applet_scopes_cover_subscribed_topics() {
        // Guard the hand-maintained identity SCOPES / TOPICS lists: every
        // subscribed topic must be granted by a requested scope, or the daemon
        // refuses the whole stream with `403 scope_denied`. The mapping lives
        // in super-stt-shared and is pinned to the daemon's `Topic::required_scope`.
        assert_eq!(
            super_stt_shared::daemon::widget_subscription::uncovered_topic(
                identity::SCOPES,
                identity::TOPICS,
            ),
            None,
        );
    }

    #[test]
    fn blocked_maps_to_widget_blocked_with_reason() {
        let update = WidgetSubscriptionUpdate::Blocked {
            reason: "auth_denied (user_denied_cached)".to_string(),
        };
        match applet_subscription_update_to_message(update) {
            Message::WidgetBlocked(reason) => {
                assert_eq!(reason, "auth_denied (user_denied_cached)");
            }
            other => panic!("Blocked must map to Message::WidgetBlocked, got {other:?}"),
        }
    }

    #[test]
    fn needs_reauth_maps_to_widget_revoked_with_reason() {
        let update = WidgetSubscriptionUpdate::NeedsReauth {
            reason: "invalid_session (expired)".to_string(),
        };
        match applet_subscription_update_to_message(update) {
            Message::WidgetRevoked(reason) => {
                assert_eq!(reason, "invalid_session (expired)");
            }
            other => panic!("NeedsReauth must map to Message::WidgetRevoked, got {other:?}"),
        }
    }

    #[test]
    fn connected_maps_to_daemon_connected_for_state_clear() {
        // Critical regression guard: an earlier bug had this mapping
        // to a no-op `WidgetOtherEvent`, which left a stale
        // `Error("revoked: …")` banner up after auto-recovery.
        let update = WidgetSubscriptionUpdate::Connected;
        assert!(matches!(
            applet_subscription_update_to_message(update),
            Message::DaemonConnected
        ));
    }

    #[test]
    fn disconnected_maps_to_subscription_error() {
        let update = WidgetSubscriptionUpdate::Disconnected {
            reason: "stream ended".to_string(),
        };
        match applet_subscription_update_to_message(update) {
            Message::WidgetSubscriptionError(reason) => {
                assert_eq!(reason, "stream ended");
            }
            other => {
                panic!("Disconnected must map to Message::WidgetSubscriptionError, got {other:?}")
            }
        }
    }

    #[test]
    fn revoked_widget_event_maps_to_widget_revoked() {
        let evt = WidgetEvent {
            name: "revoked".to_string(),
            payload: serde_json::json!({ "reason": "exe_changed" }),
        };
        match widget_event_to_message(evt) {
            Message::WidgetRevoked(reason) => assert_eq!(reason, "exe_changed"),
            other => panic!("revoked event must map to WidgetRevoked, got {other:?}"),
        }
    }
}
