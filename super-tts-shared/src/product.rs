// SPDX-License-Identifier: GPL-3.0-only
//! Super TTS's names: everything the daemon and its clients meet on, from the
//! socket to the scopes a token can carry. See
//! `super_engine_protocol::ProductSpec`.
//!
//! [`SUPER_TTS`] is defined in `super-tts-registry-types`, where the
//! installer can reach it; the tests of what Super TTS shipped live here.

pub use super_tts_registry_types::product::SUPER_TTS;

#[cfg(test)]
mod tests {
    use super::SUPER_TTS;
    use super_engine_protocol::scopes::{CORE_TOPICS, is_known_scope};

    /// The derived names are the ones Super TTS has always shipped with. A
    /// change here moves a socket, a keyring entry or an override variable
    /// out from under every installed client.
    #[test]
    fn derived_names_match_what_super_tts_shipped() {
        assert_eq!(SUPER_TTS.socket_file(), "super-tts-http.sock");
        assert_eq!(SUPER_TTS.session_keyring_service(), "super-tts-session");
        assert_eq!(SUPER_TTS.http_host(), "tts.local");
        assert_eq!(SUPER_TTS.env("HTTP_SOCKET"), "SUPER_TTS_HTTP_SOCKET");
        assert_eq!(SUPER_TTS.consent_helper(), "super-tts-consent");
    }

    /// Every topic names a scope the daemon understands, or no token could
    /// ever subscribe to it.
    #[test]
    fn every_topic_needs_a_known_scope() {
        for (topic, scope) in CORE_TOPICS.iter().chain(SUPER_TTS.topics) {
            assert!(
                is_known_scope(&SUPER_TTS, scope),
                "{topic} needs {scope}, which is not a known scope"
            );
        }
    }
}
