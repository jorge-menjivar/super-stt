// SPDX-License-Identifier: GPL-3.0-only
//! Super Voice's names: everything the daemon and its clients meet on, from
//! the socket to the scopes a token can carry. See
//! `super_engine_protocol::ProductSpec`.
//!
//! [`SUPER_VOICE`] is defined in `super-voice-registry-types`, where the
//! installer can reach it; the tests of the names its clients rely on live
//! here.

pub use super_voice_registry_types::product::SUPER_VOICE;

#[cfg(test)]
mod tests {
    use super::SUPER_VOICE;
    use super_engine_protocol::scopes::{CORE_TOPICS, is_known_scope};

    /// The names every client finds the daemon by. A change here moves a
    /// socket or an override variable out from under every installed client.
    #[test]
    fn derived_names_are_super_voice_s() {
        assert_eq!(SUPER_VOICE.socket_file(), "super-voice-http.sock");
        assert_eq!(SUPER_VOICE.http_host(), "voice.local");
        assert_eq!(SUPER_VOICE.env("HTTP_SOCKET"), "SUPER_VOICE_HTTP_SOCKET");
        assert_eq!(SUPER_VOICE.consent_helper(), "super-voice-consent");
        assert_eq!(
            super_engine_protocol::runtime::secure_runtime_path(&SUPER_VOICE, "x")
                .parent()
                .and_then(|dir| dir.file_name())
                .and_then(|name| name.to_str()),
            Some("voice"),
            "the socket lives under <runtime>/voice/"
        );
    }

    /// The macOS `LaunchAgent` plists name their log file in the variable
    /// `logging::init` reads. The plists are text, so nothing else catches a
    /// rename on one side only.
    #[test]
    fn launch_agents_set_the_log_file_variable() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap();
        let key = format!("<key>{}</key>", SUPER_VOICE.env("LOG_FILE"));
        for plist in [
            "super-voice-daemon/launchd/ai.menjivar.super-stt.plist",
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
        for (topic, scope) in CORE_TOPICS.iter().chain(SUPER_VOICE.topics) {
            assert!(
                is_known_scope(&SUPER_VOICE, scope),
                "{topic} needs {scope}, which is not a known scope"
            );
        }
    }
}
