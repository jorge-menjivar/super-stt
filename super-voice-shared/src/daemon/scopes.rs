// SPDX-License-Identifier: GPL-3.0-only
//! Super Voice's auth scope catalog: the scopes every daemon understands
//! ([`super_engine_protocol::scopes::CORE_SCOPES`]) plus Super Voice's own,
//! for both of its features.
//! Shared so the daemon (which validates `/auth/request`) and the consent
//! dialog (which describes each scope to the user) can't drift. When they
//! drift, a scope the daemon accepts but the consent binary doesn't recognize
//! renders the "unknown scope — deny is safe" warning on a legitimate prompt,
//! teaching users to distrust real requests.

use super_engine_protocol::scopes;

use crate::SUPER_VOICE;

/// Every scope token the daemon understands, in wire (`snake_case`) form. A
/// token may be granted any non-empty subset. Source of truth for
/// `/auth/request` validation and the consent dialog; mirrors the scope
/// catalog in `docs/protocol/auth.md`.
pub fn known_scopes() -> impl Iterator<Item = &'static str> {
    scopes::known_scopes(&SUPER_VOICE)
}

/// True if `s` is a recognized scope token.
#[must_use]
pub fn is_known_scope(s: &str) -> bool {
    scopes::is_known_scope(&SUPER_VOICE, s)
}

#[cfg(test)]
mod tests {
    use super::{is_known_scope, known_scopes};

    /// The whole catalog, spelled out: one list for the one product, so a
    /// scope added or dropped is a decision someone made here.
    #[test]
    fn the_catalog_is_super_voice_s() {
        let mut known: Vec<&str> = known_scopes().collect();
        known.sort_unstable();
        assert_eq!(
            known,
            [
                "audio_visualization",
                "daemon_status",
                "global_transcriptions",
                "playback_events",
                "recording_events",
                "secrets",
                "settings",
                "speak",
                "status",
                "transcribe",
                "voices",
            ]
        );
    }

    #[test]
    fn old_personas_and_garbage_are_rejected() {
        for s in ["client", "widget", "", "Settings", "transcribe ", "global"] {
            assert!(!is_known_scope(s), "{s:?} must not be a known scope");
        }
    }
}
