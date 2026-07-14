// SPDX-License-Identifier: GPL-3.0-only
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use zbus::{Connection, interface, object_server::SignalEmitter};

/// D-Bus interface for Super STT service
#[derive(Debug, Serialize, Deserialize, zbus::zvariant::Type)]
pub struct ListeningEvent {
    pub client_id: String,
    pub timestamp: String,
    pub write_mode: bool,
    pub timeout_seconds: u64,
    pub audio_level: f32,
}

#[derive(Debug, Serialize, Deserialize, zbus::zvariant::Type)]
pub struct ListeningStoppedEvent {
    pub client_id: String,
    pub timestamp: String,
    pub transcription_success: bool,
    pub error: String,
}

#[derive(Debug, Serialize, Deserialize, zbus::zvariant::Type)]
pub struct TranscriptionStartedEvent {
    pub client_id: String,
    pub timestamp: String,
    pub audio_length_ms: f64,
    pub sample_rate: u32,
}

#[derive(Debug, Serialize, Deserialize, zbus::zvariant::Type)]
pub struct TranscriptionCompletedEvent {
    pub client_id: String,
    pub timestamp: String,
    pub transcription: String,
    pub duration_ms: u64,
}

#[derive(Debug, Serialize, Deserialize, zbus::zvariant::Type)]
pub struct AudioLevelEvent {
    pub client_id: String,
    pub timestamp: String,
    pub level: f32,
    pub is_speech: bool,
}

pub struct SuperSTTDBusService;

#[interface(name = "com.github.jorge_menjivar.SuperSTT1")]
impl SuperSTTDBusService {
    /// Signal emitted when STT starts listening
    #[zbus(signal)]
    pub async fn listening_started(
        ctxt: &SignalEmitter<'_>,
        event: ListeningEvent,
    ) -> zbus::Result<()>;

    /// Signal emitted when STT stops listening
    #[zbus(signal)]
    pub async fn listening_stopped(
        ctxt: &SignalEmitter<'_>,
        event: ListeningStoppedEvent,
    ) -> zbus::Result<()>;

    /// Signal emitted when transcription starts
    #[zbus(signal)]
    pub async fn transcription_started(
        ctxt: &SignalEmitter<'_>,
        event: TranscriptionStartedEvent,
    ) -> zbus::Result<()>;

    /// Signal emitted when transcription completes
    #[zbus(signal)]
    pub async fn transcription_completed(
        ctxt: &SignalEmitter<'_>,
        event: TranscriptionCompletedEvent,
    ) -> zbus::Result<()>;

    /// Signal emitted for real-time audio level updates
    #[zbus(signal)]
    pub async fn audio_level(ctxt: &SignalEmitter<'_>, event: AudioLevelEvent) -> zbus::Result<()>;

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

/// Object path the interface is served at (and the target of every signal).
const OBJECT_PATH: &str = "/com/github/jorge_menjivar/SuperSTT";

/// Define an `emit_*` wrapper that looks up the served interface and fires one
/// of the `#[zbus(signal)]` methods. The five wrappers differ only in the
/// signal method and its event type, so generate them from one template. A
/// generic async helper can't express this cleanly (the closure would return a
/// future borrowing the emitter), so a macro is the idiomatic dedup.
macro_rules! emit_signal {
    ($(#[$meta:meta])* $method:ident => $signal:ident($event:ty)) => {
        $(#[$meta])*
        ///
        /// # Errors
        /// Returns an error if the signal cannot be emitted.
        pub async fn $method(&self, event: $event) -> Result<()> {
            let object_server = self.connection.object_server();
            let iface_ref = object_server
                .interface::<_, SuperSTTDBusService>(OBJECT_PATH)
                .await?;
            SuperSTTDBusService::$signal(iface_ref.signal_emitter(), event).await?;
            Ok(())
        }
    };
}

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
            .request_name("com.github.jorge_menjivar.SuperSTT")
            .await?;

        // Serve the interface
        connection
            .object_server()
            .at(OBJECT_PATH, SuperSTTDBusService)
            .await?;

        Ok(Self { connection })
    }

    emit_signal!(
        /// Emit a signal indicating that listening has started.
        emit_listening_started => listening_started(ListeningEvent)
    );

    emit_signal!(
        /// Emit a signal indicating that listening has stopped.
        emit_listening_stopped => listening_stopped(ListeningStoppedEvent)
    );

    emit_signal!(
        /// Emit a signal indicating that transcription has started.
        emit_transcription_started => transcription_started(TranscriptionStartedEvent)
    );

    emit_signal!(
        /// Emit a signal indicating that transcription has completed.
        emit_transcription_completed => transcription_completed(TranscriptionCompletedEvent)
    );

    emit_signal!(
        /// Emit a signal for real-time audio level updates.
        emit_audio_level => audio_level(AudioLevelEvent)
    );

    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}
