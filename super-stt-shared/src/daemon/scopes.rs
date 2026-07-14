// SPDX-License-Identifier: GPL-3.0-only
//! The auth scope catalog, shared so the daemon (which validates
//! `/auth/request`) and the consent dialog (which describes each scope to the
//! user) can't drift. When they drift, a scope the daemon accepts but the
//! consent binary doesn't recognize renders the "unknown scope — deny is safe"
//! warning on a legitimate prompt, teaching users to distrust real requests.

/// The complete set of scope tokens the daemon understands, in wire
/// (`snake_case`) form. A token may be granted any non-empty subset. Source of
/// truth for `/auth/request` validation and the consent dialog; mirrors the
/// scope catalog in `docs/protocol/auth.md`.
pub const KNOWN_SCOPES: &[&str] = &[
    "transcribe",
    "settings",
    "secrets",
    "status",
    "recording_events",
    "audio_visualization",
    "global_transcriptions",
    "daemon_status",
];

/// True if `s` is a recognized scope token.
#[must_use]
pub fn is_known_scope(s: &str) -> bool {
    KNOWN_SCOPES.contains(&s)
}

#[cfg(test)]
mod tests {
    use super::{KNOWN_SCOPES, is_known_scope};

    #[test]
    fn known_scopes_are_recognized() {
        for s in KNOWN_SCOPES {
            assert!(is_known_scope(s), "{s} should be a known scope");
        }
        assert!(
            is_known_scope("secrets"),
            "secrets must be an accepted scope"
        );
    }

    #[test]
    fn old_personas_and_garbage_are_rejected() {
        for s in ["client", "widget", "", "Settings", "transcribe ", "global"] {
            assert!(!is_known_scope(s), "{s:?} must not be a known scope");
        }
    }
}
