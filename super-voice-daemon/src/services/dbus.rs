// SPDX-License-Identifier: GPL-3.0-only
//! The daemon's session-bus presence: a name to own and a liveness probe.
//!
//! This interface used to publish five signals: listening started/stopped,
//! transcription started/completed, and audio level. `transcription_completed`
//! carried the full text of every transcription. They are deliberately gone: a
//! session-bus signal is readable by every process on the bus, with no consent
//! at all, while the same information on `GET /v1/events` costs a
//! `recording_events`, `global_transcriptions` or `audio_visualization` grant
//! the user has to approve. Publishing both made those grants a formality, since
//! anything denied there could just listen on the bus instead. No client
//! listened to them anyway: every first-party client uses `/v1/events`.
//!
//! What is left is what carries no information the user would want gated: the
//! well-known name (so "is the daemon up?" is answerable without a socket) and
//! a ping. Super TTS made the same cut.
//!
//! **Linux only.** macOS has no session bus, so the service is not served there.

#[cfg(target_os = "linux")]
use anyhow::Result;
#[cfg(target_os = "linux")]
use std::collections::HashMap;
#[cfg(target_os = "linux")]
use zbus::{Connection, interface};

#[cfg(target_os = "linux")]
pub struct SuperVoiceDBusService;

#[cfg(target_os = "linux")]
#[interface(name = "com.github.jorge_menjivar.SuperVoice1")]
impl SuperVoiceDBusService {
    /// Method to check if daemon is running
    #[must_use]
    pub fn ping(&self) -> String {
        "pong".to_string()
    }

    /// Method to get current listening status
    #[must_use]
    pub fn get_status(&self) -> HashMap<String, String> {
        let mut status = HashMap::new();
        status.insert("service".to_string(), "running".to_string());
        status.insert("version".to_string(), "0.1.0".to_string());
        status
    }
}

/// Object path the interface is served at.
#[cfg(target_os = "linux")]
const OBJECT_PATH: &str = "/com/github/jorge_menjivar/SuperVoice";

#[cfg(target_os = "linux")]
pub struct DBusManager {
    connection: Connection,
}

#[cfg(target_os = "linux")]
impl DBusManager {
    /// Create a new `DBusManager` instance.
    ///
    /// # Errors
    /// This function will return an error if the connection to the session bus cannot be established.
    pub async fn new() -> Result<Self> {
        let connection = Connection::session().await?;

        // Request the service name
        connection
            .request_name("com.github.jorge_menjivar.SuperVoice")
            .await?;

        // Serve the interface
        connection
            .object_server()
            .at(OBJECT_PATH, SuperVoiceDBusService)
            .await?;

        Ok(Self { connection })
    }

    // With a reason: clippy 1.99 asks for `#[must_use]` here, and 1.98 refuses
    // a bare one, since it takes `&Connection` as `#[must_use]` already.
    #[must_use = "this only borrows the connection"]
    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}

/// The macOS stand-in for [the Linux `DBusManager`](DBusManager).
///
/// Uninhabited on purpose. The daemon holds its manager as an
/// `Option<Arc<DBusManager>>`; making the type impossible to construct turns
/// "there is no session bus on macOS" into something the compiler knows, so
/// that field is `None` by construction rather than by convention.
#[cfg(target_os = "macos")]
pub enum DBusManager {}
