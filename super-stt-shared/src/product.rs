// SPDX-License-Identifier: GPL-3.0-only
//! Super STT's names: everything the daemon and its clients meet on, from the
//! socket to the scopes a token can carry. See
//! `super_engine_protocol::ProductSpec`.
//!
//! [`SUPER_STT`] is defined in `super-stt-registry-types`, where the
//! installer can reach it; the tests of what Super STT shipped live here.

pub use super_stt_registry_types::product::SUPER_STT;

#[cfg(test)]
mod tests {
    use super::SUPER_STT;
    use super_engine_protocol::scopes::{CORE_TOPICS, is_known_scope};

    /// The derived names are the ones Super STT has always shipped with. A
    /// change here moves a socket, a keyring entry or an override variable
    /// out from under every installed client.
    #[test]
    fn derived_names_match_what_super_stt_shipped() {
        assert_eq!(SUPER_STT.socket_file(), "super-stt-http.sock");
        assert_eq!(SUPER_STT.session_keyring_service(), "super-stt-session");
        assert_eq!(SUPER_STT.http_host(), "stt.local");
        assert_eq!(SUPER_STT.env("HTTP_SOCKET"), "SUPER_STT_HTTP_SOCKET");
        assert_eq!(SUPER_STT.consent_helper(), "super-stt-consent");
    }

    /// The macOS `LaunchAgent` plists name their log file in the variable
    /// `logging::init` reads. The plists are text, so nothing else catches a
    /// rename on one side only.
    #[test]
    fn launch_agents_set_the_log_file_variable() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap();
        let key = format!("<key>{}</key>", SUPER_STT.env("LOG_FILE"));
        for plist in [
            "super-stt-daemon/launchd/ai.menjivar.super-stt.plist",
            "super-stt-cli/launchd/ai.menjivar.super-stt.hotkey.plist",
        ] {
            let text = std::fs::read_to_string(root.join(plist)).unwrap();
            assert!(text.contains(&key), "{plist} does not set {key}");
        }
    }

    /// Every topic names a scope the daemon understands, or no token could
    /// ever subscribe to it.
    #[test]
    fn every_topic_needs_a_known_scope() {
        for (topic, scope) in CORE_TOPICS.iter().chain(SUPER_STT.topics) {
            assert!(
                is_known_scope(&SUPER_STT, scope),
                "{topic} needs {scope}, which is not a known scope"
            );
        }
    }
}
