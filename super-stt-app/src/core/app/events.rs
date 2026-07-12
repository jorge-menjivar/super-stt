// SPDX-License-Identifier: GPL-3.0-only

use crate::state::DaemonStatus;
use crate::ui::messages::Message;
use log::warn;

/// Classify a daemon [`HttpError`] into the right next `DaemonStatus`.
///
/// A user-triggered `auth_denied` ([`is_user_denied`]) must transition to
/// [`DaemonStatus::Blocked`] so the surrounding auto-retry loop stops firing —
/// otherwise the settings app re-pings every 5s and the daemon's in-memory deny
/// cache keeps logging the same `user_denied_cached`. Every other error is
/// transient (daemon restarting, socket missing, token expiry, …) and gets
/// [`DaemonStatus::Error`], which the caller pairs with the 5s retry. The
/// decision is on the typed variant; the human string is kept only for display.
///
/// [`HttpError`]: super_stt_shared::daemon::http_client::HttpError
/// [`is_user_denied`]: super_stt_shared::daemon::widget_subscription::is_user_denied
pub(super) fn classify_daemon_error(
    err: super_stt_shared::daemon::http_client::HttpError,
) -> DaemonStatus {
    if super_stt_shared::daemon::widget_subscription::is_user_denied(&err) {
        DaemonStatus::Blocked(err.into())
    } else {
        DaemonStatus::Error(err.into())
    }
}

/// Pick out the events the settings UI cares about and translate them
/// into the `Message` variants that drive the audio meter +
/// recording-status badge. Returns `None` for events we don't render
/// (e.g. `subscribed`, `error`, `revoked`).
pub(super) fn settings_widget_event_to_message(
    evt: &super_stt_shared::daemon::http_client::WidgetEvent,
) -> Option<Message> {
    use serde_json::Value;
    let p: &Value = &evt.payload;
    match evt.name.as_str() {
        "recording_state" => Some(Message::WidgetRecordingState(
            p.get("is_recording")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        )),
        "frequency_bands" => {
            // reason: audio energy values are small positive floats well within f32 range; precision loss is acceptable for level display.
            #[allow(clippy::cast_possible_truncation)]
            let total_energy = p.get("total_energy").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let level = raw_level_to_db_display_percent(total_energy);
            let is_speech = total_energy > 0.0001;
            Some(Message::WidgetAudioLevel { level, is_speech })
        }
        // `daemon_status_changed` and `download_progress` arrive as
        // settings-scope SSE topics now; the existing
        // `Message::DaemonEventsReceived` handler already knows how to
        // dispatch the legacy JSON shape, so we wrap each event in a
        // singleton `NotificationEvent` and feed it through unchanged.
        "daemon_status_changed" | "download_progress" => {
            widget_event_to_notification(evt).map(|n| Message::DaemonEventsReceived(vec![n]))
        }
        "registry_install" => {
            use super_stt_shared::registry::events::RegistryEvent;
            let p = &evt.payload;
            if let Ok(reg_evt) = serde_json::from_value::<RegistryEvent>(p.clone()) {
                match reg_evt {
                    RegistryEvent::Progress {
                        install_id,
                        source,
                        phase,
                        bytes_done,
                        bytes_total,
                    } => Some(Message::InstallProgress {
                        install_id,
                        source,
                        phase,
                        bytes_done,
                        bytes_total,
                    }),
                    RegistryEvent::Completed { source, .. } => {
                        Some(Message::InstallCompleted { source })
                    }
                    RegistryEvent::Failed {
                        install_id,
                        source,
                        phase,
                        error,
                    } => Some(Message::InstallFailed {
                        install_id,
                        source,
                        phase,
                        error,
                    }),
                    RegistryEvent::RefreshCompleted { .. }
                    | RegistryEvent::RefreshFailed { .. } => None,
                }
            } else {
                warn!(
                    "registry_install event failed to deserialize as RegistryEvent — \
                     dropping (payload: {p})"
                );
                None
            }
        }
        _ => None,
    }
}

/// Wrap a settings-scope SSE event in the legacy `NotificationEvent`
/// shape so the long-standing `Message::DaemonEventsReceived` handler
/// keeps working without restructuring.
///
/// Returns `None` when the event is malformed — specifically, when
/// the `timestamp` field is missing or not a string. Per the daemon's
/// wire contract every `daemon_status_changed` / `download_progress`
/// publish includes a string `timestamp`, so its absence is a
/// protocol violation and the event should be dropped rather than
/// patched up with a fake clock value.
pub(super) fn widget_event_to_notification(
    evt: &super_stt_shared::daemon::http_client::WidgetEvent,
) -> Option<super_stt_shared::models::protocol::NotificationEvent> {
    let Some(timestamp) = evt
        .payload
        .get("timestamp")
        .and_then(serde_json::Value::as_str)
    else {
        warn!(
            "widget event '{}' missing string `timestamp` field — \
             dropping malformed event (payload: {})",
            evt.name, evt.payload
        );
        return None;
    };
    Some(super_stt_shared::models::protocol::NotificationEvent {
        event_type_field: evt.name.clone(),
        event_type: evt.name.clone(),
        client_id: "daemon".to_string(),
        timestamp: timestamp.to_owned(),
        data: evt.payload.clone(),
    })
}

/// Convert raw frequency-band energy (typically 0.00001-0.1) into a
/// 0.0-1.0 display percentage via a -60 dB ... 0 dB log mapping. Same
/// transform the legacy UDP path applied in `audio/networking.rs`.
pub(super) fn raw_level_to_db_display_percent(raw_level: f32) -> f32 {
    let db = if raw_level <= 0.0 {
        -60.0
    } else {
        // Same scaling used in the legacy UDP path: map quiet/normal/loud
        // speech (0.003 / 0.005 / 0.008) to ~80-97% display.
        (20.0 * (raw_level * 10.0).log10()).clamp(-60.0, 0.0)
    };
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
}

#[cfg(test)]
mod classify_daemon_error_tests {
    //! Pin the decision in `Message::DaemonError`: which error
    //! strings should suppress the 5s auto-retry vs. trigger it.
    //! Locking this in protects against a regression of the deny
    //! spam loop the helper change already fixed for the applet.
    use super::*;
    use super_stt_shared::daemon::http_client::HttpError;

    fn auth_denied(reason: &str) -> HttpError {
        HttpError::AuthDenied {
            reason: reason.to_string(),
        }
    }

    #[test]
    fn user_denied_cached_routes_to_blocked() {
        let next = classify_daemon_error(auth_denied("user_denied_cached"));
        match next {
            DaemonStatus::Blocked(reason) => {
                // Blocked keeps the human string for display.
                assert_eq!(reason, "auth_denied (user_denied_cached)");
            }
            other => panic!("user_denied_cached must route to Blocked, got {other:?}"),
        }
    }

    #[test]
    fn fresh_user_denied_routes_to_blocked() {
        let next = classify_daemon_error(auth_denied("user_denied"));
        assert!(matches!(next, DaemonStatus::Blocked(_)));
    }

    #[test]
    fn dismissed_popup_routes_to_error_so_retry_can_recover() {
        // user_dismissed is recoverable — next attempt pops the
        // popup fresh — so the retry loop must keep firing.
        let next = classify_daemon_error(auth_denied("user_dismissed"));
        assert!(matches!(next, DaemonStatus::Error(_)));
    }

    #[test]
    fn invalid_session_routes_to_error() {
        // Token expiry / exe_changed are transient — let the
        // retry loop drive a fresh consent on the next attempt.
        let next = classify_daemon_error(HttpError::InvalidSession {
            reason: "expired".to_string(),
        });
        assert!(matches!(next, DaemonStatus::Error(_)));
    }

    #[test]
    fn socket_unreachable_routes_to_error() {
        // Daemon restarting / socket missing — pure transient.
        let next = classify_daemon_error(HttpError::Other(
            "Daemon HTTP listener not running. Start the daemon first.".to_string(),
        ));
        assert!(matches!(next, DaemonStatus::Error(_)));
    }
}

#[cfg(test)]
mod widget_event_to_notification_tests {
    //! Pin the contract that malformed events (missing or non-string
    //! `timestamp`) are dropped rather than patched up with a
    //! consumer-side clock value.
    use super::*;
    use super_stt_shared::daemon::http_client::WidgetEvent;

    #[test]
    fn well_formed_event_produces_notification() {
        let evt = WidgetEvent {
            name: "daemon_status_changed".to_string(),
            payload: serde_json::json!({
                "status": "ready",
                "model_loaded": true,
                "timestamp": "2026-05-22T12:35:14Z",
            }),
        };
        let n = widget_event_to_notification(&evt).expect("should produce a NotificationEvent");
        assert_eq!(n.event_type, "daemon_status_changed");
        assert_eq!(n.timestamp, "2026-05-22T12:35:14Z");
        assert_eq!(n.data["status"], "ready");
    }

    #[test]
    fn missing_timestamp_is_dropped() {
        let evt = WidgetEvent {
            name: "daemon_status_changed".to_string(),
            payload: serde_json::json!({ "status": "ready" }),
        };
        assert!(widget_event_to_notification(&evt).is_none());
    }

    #[test]
    fn non_string_timestamp_is_dropped() {
        let evt = WidgetEvent {
            name: "download_progress".to_string(),
            payload: serde_json::json!({
                "percentage": 25.0,
                "timestamp": 1_716_393_314_i64,
            }),
        };
        assert!(widget_event_to_notification(&evt).is_none());
    }
}
