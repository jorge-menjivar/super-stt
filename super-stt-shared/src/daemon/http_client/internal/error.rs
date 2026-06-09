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
        // `session::with_token` recognizes the InvalidSession retry
        // signal by matching the `Display` prefix
        // `invalid_session (`. If this format ever changes the retry
        // stops firing — so this test pins it down.
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
