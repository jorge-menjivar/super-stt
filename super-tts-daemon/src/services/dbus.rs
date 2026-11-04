// SPDX-License-Identifier: GPL-3.0-only
//! The daemon's session-bus presence: a name to own and a liveness probe.
//!
//! The STT build also published five signals here — listening started/stopped,
//! transcription started/completed, audio level — and this daemon has the
//! playback equivalents of all of them. They are deliberately **not** mirrored
//! onto D-Bus: a session-bus signal is readable by every process on the bus,
//! while the same information on `GET /events` costs a `playback_events` grant
//! the user has to approve. Publishing both would make the consent prompt a
//! formality, since anything denied there could just listen on the bus instead.
//!
//! What is left is what carries no information the user would want gated: the
//! well-known name (so "is the daemon up?" is answerable without a socket) and
//! a ping.

use anyhow::Result;
use std::collections::HashMap;
use zbus::{Connection, interface};

pub struct SuperTTSDBusService;

#[interface(name = "com.github.jorge_menjivar.SuperTTS1")]
impl SuperTTSDBusService {
    /// Method to check if daemon is running
    #[must_use]
    pub fn ping(&self) -> String {
        "pong".to_string()
    }

    /// Method to get the service's coarse status.
    ///
    /// Deliberately coarse: whether the daemon is *speaking*, and which model
    /// it loaded, live behind the `status` scope on `GET /v1/status`. This
    /// answers only "something is serving this name".
    #[must_use]
    pub fn get_status(&self) -> HashMap<String, String> {
        let mut status = HashMap::new();
        status.insert("service".to_string(), "running".to_string());
        status.insert("version".to_string(), env!("CARGO_PKG_VERSION").to_string());
        status
    }
}

/// Object path the interface is served at.
const OBJECT_PATH: &str = "/com/github/jorge_menjivar/SuperTTS";

pub struct DBusManager {
    connection: Connection,
}

impl DBusManager {
    /// Create a new `DBusManager` instance.
    ///
    /// # Errors
    /// This function will return an error if the connection to the session bus cannot be established.
    pub async fn new() -> Result<Self> {
        let connection = Connection::session().await?;

        // Request the service name
        connection
            .request_name("com.github.jorge_menjivar.SuperTTS")
            .await?;

        // Serve the interface
        connection
            .object_server()
            .at(OBJECT_PATH, SuperTTSDBusService)
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
