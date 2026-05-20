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
            .at("/com/github/jorge_menjivar/SuperSTT", SuperSTTDBusService)
            .await?;

        Ok(Self { connection })
    }

    /// Emit a signal indicating that listening has started.
    ///
    /// # Errors
    /// This function will return an error if the signal cannot be emitted.
    pub async fn emit_listening_started(&self, event: ListeningEvent) -> Result<()> {
        let object_server = self.connection.object_server();
        let iface_ref = object_server
            .interface::<_, SuperSTTDBusService>("/com/github/jorge_menjivar/SuperSTT")
            .await?;

        SuperSTTDBusService::listening_started(iface_ref.signal_emitter(), event).await?;
        Ok(())
    }

    /// Emit a signal indicating that listening has stopped.
    ///
    /// # Errors
    /// This function will return an error if the signal cannot be emitted.
    pub async fn emit_listening_stopped(&self, event: ListeningStoppedEvent) -> Result<()> {
        let object_server = self.connection.object_server();
        let iface_ref = object_server
            .interface::<_, SuperSTTDBusService>("/com/github/jorge_menjivar/SuperSTT")
            .await?;

        SuperSTTDBusService::listening_stopped(iface_ref.signal_emitter(), event).await?;
        Ok(())
    }

    /// Emit a signal indicating that transcription has started.
    ///
    /// # Errors
    /// This function will return an error if the signal cannot be emitted.
    pub async fn emit_transcription_started(&self, event: TranscriptionStartedEvent) -> Result<()> {
        let object_server = self.connection.object_server();
        let iface_ref = object_server
            .interface::<_, SuperSTTDBusService>("/com/github/jorge_menjivar/SuperSTT")
            .await?;

        SuperSTTDBusService::transcription_started(iface_ref.signal_emitter(), event).await?;
        Ok(())
    }

    /// Emit a signal indicating that transcription has completed.
    ///
    /// # Errors
    /// This function will return an error if the signal cannot be emitted.
    pub async fn emit_transcription_completed(
        &self,
        event: TranscriptionCompletedEvent,
    ) -> Result<()> {
        let object_server = self.connection.object_server();
        let iface_ref = object_server
            .interface::<_, SuperSTTDBusService>("/com/github/jorge_menjivar/SuperSTT")
            .await?;

        SuperSTTDBusService::transcription_completed(iface_ref.signal_emitter(), event).await?;
        Ok(())
    }

    /// Emit a signal for real-time audio level updates.
    ///
    /// # Errors
    /// This function will return an error if the signal cannot be emitted.
    pub async fn emit_audio_level(&self, event: AudioLevelEvent) -> Result<()> {
        let object_server = self.connection.object_server();
        let iface_ref = object_server
            .interface::<_, SuperSTTDBusService>("/com/github/jorge_menjivar/SuperSTT")
            .await?;

        SuperSTTDBusService::audio_level(iface_ref.signal_emitter(), event).await?;
        Ok(())
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}
