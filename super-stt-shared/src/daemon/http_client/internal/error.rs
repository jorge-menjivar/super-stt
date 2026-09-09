// SPDX-License-Identifier: GPL-3.0-only
/// Errors returned by every HTTP-protocol call. The `Display` impl
/// reproduces the legacy `String` error wording so existing UI
/// error-toast plumbing keeps working unchanged.
#[derive(Debug, Clone)]
pub enum HttpError {
    /// Daemon rejected the bearer token. Mirrors the 401
    /// `{ "message": "invalid_session", "data": { "reason": ... } }`
    /// response body. Callers should drop the cached token
    /// (`session::forget`) and re-`obtain`.
    InvalidSession {
        /// Daemon-supplied reason: `unknown`, `expired`, `exe_changed`.
        reason: String,
    },
    /// `POST /auth/request` denied. Mirrors the 403 `auth_denied` body.
    /// Reasons: `user_denied`, `user_denied_cached`, `user_dismissed`,
    /// `popup_failed`, `invalid_scope`, `throttled`, …
    AuthDenied {
        /// Daemon-supplied reason — see [`auth.md`].
        ///
        /// [`auth.md`]: ../../../docs/protocol/auth.md
        reason: String,
    },
    /// Anything else: daemon unreachable, malformed body, transport
    /// error, daemon-returned `{"status":"error",…}` body without a
    /// recognized identifier, etc.
    Other(String),
}

impl HttpError {
    /// True if the error means "your bearer token is no longer good"
    /// — the only condition that should trigger a re-`obtain`.
    /// Convenience helper for the small handful of retry-on-401 sites.
    #[must_use]
    pub const fn is_invalid_session(&self) -> bool {
        matches!(self, Self::InvalidSession { .. })
    }

    /// The human half, for text put in front of a person.
    ///
    /// [`Display`](std::fmt::Display) is shaped for logs: it carries the
    /// daemon's stable error token and the HTTP status beside the message,
    /// which is what made issue #423 diagnosable from a log alone. Neither
    /// belongs in a drawer — `not_found: no published release at ... (HTTP
    /// 404)` reads as a stack trace to the person who just pasted a URL. This
    /// drops exactly the decorations
    /// [`daemon_error`](super::transport::daemon_error) adds and returns the
    /// sentence the daemon wrote.
    ///
    /// A message that carries no such decoration is returned unchanged, so an
    /// error from anywhere else still reads as itself.
    #[must_use]
    pub fn user_message(&self) -> String {
        match self {
            Self::InvalidSession { .. } => {
                "The session with the Super STT service expired. Reconnecting.".to_string()
            }
            Self::AuthDenied { .. } => {
                "Super STT did not grant this app permission for that.".to_string()
            }
            Self::Other(s) => strip_log_decoration(s),
        }
    }
}

/// Remove the ` (HTTP nnn)` suffix and a leading `snake_case_token: ` prefix
/// that `daemon_error` adds, leaving the daemon's own sentence.
///
/// The prefix is only dropped when it really is one of the daemon's error
/// tokens — lowercase, no spaces — so a message that merely contains a colon
/// ("Reaches: nothing", a Windows path) survives intact.
fn strip_log_decoration(s: &str) -> String {
    let mut out = s.trim();

    if let Some(open) = out.rfind(" (HTTP ")
        && out.ends_with(')')
        && out[open + 7..out.len() - 1]
            .chars()
            .all(|c| c.is_ascii_digit())
    {
        out = out[..open].trim_end();
    }

    if let Some((head, rest)) = out.split_once(": ")
        && !head.is_empty()
        && head
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !rest.trim().is_empty()
    {
        out = rest.trim_start();
    }

    out.to_string()
}

#[cfg(test)]
mod user_message_tests {
    use super::HttpError;

    /// The exact string issue #423's drawer would have shown.
    #[test]
    fn drops_the_token_and_the_status() {
        let e = HttpError::Other(
            "not_found: no published release at `github.com/o/b`. A fork does not inherit \
             the upstream's releases (HTTP 404)"
                .to_string(),
        );
        assert_eq!(
            e.user_message(),
            "no published release at `github.com/o/b`. A fork does not inherit the \
             upstream's releases"
        );
    }

    /// The log form keeps both, because that is what made the bug reportable.
    #[test]
    fn display_is_left_alone() {
        let raw = "not_found: no release (HTTP 404)";
        assert_eq!(HttpError::Other(raw.to_string()).to_string(), raw);
    }

    #[test]
    fn a_colon_inside_the_sentence_survives() {
        for raw in [
            "Reaches: nothing at all",
            "C:/Users/alice/backend has no manifest",
            "Not Found",
        ] {
            assert_eq!(
                HttpError::Other(raw.to_string()).user_message(),
                raw,
                "{raw}"
            );
        }
    }

    #[test]
    fn a_bare_token_with_no_message_is_kept() {
        // Nothing human to fall back on, so the token is better than "".
        assert_eq!(
            HttpError::Other("registry_unavailable (HTTP 503)".to_string()).user_message(),
            "registry_unavailable"
        );
    }
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSession { reason } => write!(f, "invalid_session ({reason})"),
            Self::AuthDenied { reason } => write!(f, "auth_denied ({reason})"),
            Self::Other(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for HttpError {}

/// Internal `?`-bridge: a handful of helpers inside this crate
/// (`build_request`, `connect_socket`, …) return `Result<_, String>`
/// for transport-level failures that don't have a typed variant. Wrap
/// those in `HttpError::Other` so callers can keep using `?` against
/// `HttpResult<T>`.
///
/// **This produces `Other` only.** A `String` whose text happens to
/// match the `Display` of `InvalidSession`/`AuthDenied` (e.g. round-
/// tripping `HttpError → String → HttpError`) does NOT round-trip back
/// to the original typed variant; `is_invalid_session()` would
/// disagree with `Display`. Don't reach for this conversion as a way
/// to parse a wire string back into a typed error — construct the
/// variant directly at the production site (the `send_request` 401
/// path, the `auth_request` 4xx path) where the structured info is
/// available.
impl From<String> for HttpError {
    fn from(s: String) -> Self {
        Self::Other(s)
    }
}

/// `From<HttpError> for String` lets existing UI plumbing that uses
/// `Result<T, String>` propagate an HTTP-typed error via `?` without
/// rewriting every iced task closure. Equivalent to
/// `e.to_string()` — preserves the wire-visible message text.
impl From<HttpError> for String {
    fn from(e: HttpError) -> Self {
        e.to_string()
    }
}

/// Result type for all HTTP-protocol calls. `HttpError` formats to the
/// same string the previous `Result<T, String>` produced, so callers
/// that only care about the message text don't change.
pub type HttpResult<T> = Result<T, HttpError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_error_display_locks_wire_format() {
        // `Display` is the human string shown in UI toasts and stored in
        // `DaemonStatus::Blocked`/`Error`. The retry and blocked-vs-error
        // decisions now match the typed variant (not this text), but the wording
        // is still user-visible, so pin it.
        let e = HttpError::InvalidSession {
            reason: "expired".to_string(),
        };
        assert_eq!(e.to_string(), "invalid_session (expired)");
        assert!(e.to_string().starts_with("invalid_session ("));

        let e = HttpError::AuthDenied {
            reason: "user_denied_cached".to_string(),
        };
        assert_eq!(e.to_string(), "auth_denied (user_denied_cached)");

        // Other variants don't share the InvalidSession prefix.
        let e = HttpError::Other("Daemon HTTP listener not running.".to_string());
        assert!(!e.to_string().starts_with("invalid_session ("));
    }

    #[test]
    fn http_error_is_invalid_session_helper_matches_only_invalid_session() {
        assert!(
            HttpError::InvalidSession {
                reason: "unknown".into()
            }
            .is_invalid_session()
        );
        assert!(
            !HttpError::AuthDenied {
                reason: "user_denied".into()
            }
            .is_invalid_session()
        );
        assert!(!HttpError::Other("anything".into()).is_invalid_session());
    }
}
