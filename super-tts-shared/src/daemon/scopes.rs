// SPDX-License-Identifier: GPL-3.0-only
//! Super TTS's auth scope catalog: the scopes every daemon understands
//! ([`super_engine_protocol::scopes::CORE_SCOPES`]) plus Super TTS's own.
//! Shared so the daemon (which validates `/auth/request`) and the consent
//! dialog (which describes each scope to the user) can't drift. When they
//! drift, a scope the daemon accepts but the consent binary doesn't recognize
//! renders the "unknown scope — deny is safe" warning on a legitimate prompt,
//! teaching users to distrust real requests.

use super_engine_protocol::scopes;

use crate::SUPER_TTS;

/// Every scope token the daemon understands, in wire (`snake_case`) form. A
/// token may be granted any non-empty subset. Source of truth for
/// `/auth/request` validation and the consent dialog; mirrors the scope
/// catalog in `docs/protocol/auth.md`.
pub fn known_scopes() -> impl Iterator<Item = &'static str> {
    scopes::known_scopes(&SUPER_TTS)
}

/// True if `s` is a recognized scope token.
#[must_use]
pub fn is_known_scope(s: &str) -> bool {
    scopes::is_known_scope(&SUPER_TTS, s)
}

#[cfg(test)]
mod tests {
    use super::{is_known_scope, known_scopes};

    /// The catalog is the one Super TTS shipped before it shared the
    /// definition, so no client's token loses or gains a scope.
    #[test]
    fn the_catalog_is_the_one_super_stt_shipped() {
        let mut known: Vec<&str> = known_scopes().collect();
        known.sort_unstable();
        assert_eq!(
            known,
            [
                "audio_visualization",
                "daemon_status",
                "playback_events",
                "secrets",
                "settings",
                "speak",
                "status",
                "voices",
            ]
        );
    }

    #[test]
    fn old_personas_and_garbage_are_rejected() {
        for s in ["client", "widget", "", "Settings", "speak ", "global"] {
            assert!(!is_known_scope(s), "{s:?} must not be a known scope");
        }
    }
}
