use super::*;

// Backoff progression is now `RetryStrategy` (see `daemon::retry` tests); the
// subscription loop just drives it via `next_delay`/`should_retry`/`reset`.

#[test]
fn http_error_is_invalid_session_matches_only_invalid_session() {
    use http_client::HttpError;
    // The replacement for the old free `is_invalid_session(&str)`
    // is `HttpError::is_invalid_session()` on the typed error.
    assert!(
        HttpError::InvalidSession {
            reason: "unknown".into()
        }
        .is_invalid_session()
    );
    assert!(
        HttpError::InvalidSession {
            reason: "expired".into()
        }
        .is_invalid_session()
    );
    // Other variants must NOT classify as invalid_session.
    assert!(
        !HttpError::AuthDenied {
            reason: "user_denied".into()
        }
        .is_invalid_session()
    );
    assert!(!HttpError::Other("connection refused".into()).is_invalid_session());
}

#[test]
fn is_user_denied_reason_matches_both_deny_variants() {
    assert!(is_user_denied_reason("user_denied"));
    assert!(is_user_denied_reason("user_denied_cached"));
    // Other reasons must not.
    assert!(!is_user_denied_reason("user_dismissed"));
    assert!(!is_user_denied_reason("popup_failed"));
}

#[test]
fn is_user_denied_matches_both_reasons() {
    use crate::daemon::http_client::HttpError;
    let denied = |reason: &str| HttpError::AuthDenied {
        reason: reason.to_string(),
    };
    // Both fresh denials and deny-cache hits must terminate the
    // subscription so we don't spam the daemon.
    assert!(is_user_denied(&denied("user_denied")));
    assert!(is_user_denied(&denied("user_denied_cached")));
    // A dismissed popup is recoverable — the user just walked away,
    // so the next attempt will pop a fresh prompt. Do NOT treat as
    // blocked.
    assert!(!is_user_denied(&denied("user_dismissed")));
    // popup_failed / invalid_scope / etc. — those are infra
    // problems, the consumer's normal backoff path handles them.
    assert!(!is_user_denied(&denied("popup_failed")));
    assert!(!is_user_denied(&HttpError::InvalidSession {
        reason: "expired".to_string(),
    }));
}

/// The classifier keys off the typed [`HttpError::AuthDenied`] reason, so only
/// user-initiated denials drive the Blocked path; every other `auth_denied`
/// reason takes the ordinary backoff/reconnect path.
#[test]
fn user_denied_reasons_classify_as_blocked() {
    use crate::daemon::http_client::HttpError;
    let denied = |reason: &str| HttpError::AuthDenied {
        reason: reason.to_string(),
    };
    for reason in ["user_denied", "user_denied_cached"] {
        assert!(
            is_user_denied(&denied(reason)),
            "`{reason}` must classify as user-denied"
        );
    }
    for reason in [
        "user_dismissed",
        "popup_failed",
        "throttled",
        "invalid_scope",
    ] {
        assert!(
            !is_user_denied(&denied(reason)),
            "`{reason}` must NOT classify as user-denied"
        );
    }
}

/// Document the Blocked variant carries the daemon's reason
/// string verbatim so consumers (the applet popup, the settings
/// connection page) can surface it to the user without having
/// to parse anything out of it.
#[test]
fn blocked_update_carries_reason_string() {
    let update = WidgetSubscriptionUpdate::Blocked {
        reason: "auth_denied (user_denied_cached)".to_string(),
    };
    match update {
        WidgetSubscriptionUpdate::Blocked { reason } => {
            assert_eq!(reason, "auth_denied (user_denied_cached)");
        }
        other => panic!("variant constructed Blocked but pattern saw {other:?}"),
    }
}

/// Routing contract: which `HttpError` variant maps to which
/// `WidgetSubscriptionUpdate` variant. Pins the fix for the
/// "daemon offline reported as 'session revoked'" UX bug —
/// `HttpError::Other` must drive `Disconnected`, not
/// `NeedsReauth`. The actual routing lives inside the
/// `run_widget_subscription` match, but we can verify the
/// intent by enumerating the discriminants we expect each
/// variant to land in.
#[test]
fn obtain_error_routing_contract() {
    use http_client::HttpError;

    // The catch-all `Err(e @ HttpError::Other(_)) => Disconnected`
    // arm fires here. Failing this would mean a daemon-offline
    // state is again reported as a revoked session.
    let daemon_offline = HttpError::Other("Daemon HTTP listener not running.".to_string());
    assert!(
        !daemon_offline.is_invalid_session(),
        "Other must not be classified as invalid_session"
    );
    assert!(matches!(daemon_offline, HttpError::Other(_)));

    // `InvalidSession` and non-user-denied `AuthDenied` must
    // land in `NeedsReauth`. We can't observe the stream output
    // synchronously here, but we lock in the predicate the
    // routing relies on.
    let invalid = HttpError::InvalidSession {
        reason: "expired".to_string(),
    };
    assert!(invalid.is_invalid_session());

    let popup_failed = HttpError::AuthDenied {
        reason: "popup_failed".to_string(),
    };
    assert!(
        !matches!(&popup_failed, HttpError::AuthDenied { reason } if is_user_denied_reason(reason)),
        "popup_failed must NOT be classified as user-denied — \
         it's recoverable, not terminal"
    );

    // The terminal `Blocked` path only triggers on these two.
    for reason in ["user_denied", "user_denied_cached"] {
        let e = HttpError::AuthDenied {
            reason: reason.to_string(),
        };
        assert!(
            matches!(&e, HttpError::AuthDenied { reason } if is_user_denied_reason(reason)),
            "`{reason}` must classify as terminal user-denied"
        );
    }
}

#[test]
fn config_defaults_are_sane() {
    let cfg = WidgetSubscriptionConfig::new(
        AppId("test-app"),
        "Test App",
        &["recording_events"],
        &["recording_state"],
    );
    // Idle timeout must be ≥ 2× the daemon's keepalive interval (30 s).
    assert!(cfg.idle_timeout >= Duration::from_mins(1));
    // Backoff must grow.
    assert!(cfg.initial_backoff < cfg.max_backoff);
}
