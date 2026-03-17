// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::SuperSTTDaemon;
use anyhow::Result;
use chrono::{DateTime, Utc};
use log::{error, info, warn};
use std::collections::HashMap;
use super_stt_shared::models::protocol::{DaemonRequest, DaemonResponse};
use super_stt_shared::validation::Validate;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::broadcast;

/// Track active client connections
#[derive(Debug, Clone)]
pub struct ClientConnection {
    pub last_seen: DateTime<Utc>,
}

/// Manages active client connections
pub type ClientConnectionsMap =
    std::sync::Arc<tokio::sync::RwLock<HashMap<String, ClientConnection>>>;

impl SuperSTTDaemon {
    /// Handle client connection
    ///
    /// # Errors
    ///
    /// Returns an error if reading from or writing to the stream fails.
    pub async fn handle_client(&self, mut stream: UnixStream) -> Result<()> {
        // Generate a unique client ID for this connection
        let client_id = format!("conn_{}", uuid::Uuid::new_v4());

        // Register the connection with resource manager
        if let Err(e) = self
            .resource_manager
            .register_connection(client_id.clone(), None)
            .await
        {
            warn!("Connection rejected due to resource limits: {e}");
            let response = DaemonResponse::error(&format!("Connection rejected: {e}"));
            let _ = self.send_response(&mut stream, &response).await;
            return Ok(());
        }

        loop {
            // Read message size (8 bytes, big endian)
            let mut size_buf = [0u8; 8];
            match stream.read_exact(&mut size_buf).await {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    break;
                }
                Err(e) => {
                    warn!("Failed to read message size: {e}");
                    break;
                }
            }

            let Ok(message_size) = usize::try_from(u64::from_be_bytes(size_buf)) else {
                warn!("Invalid message size received");
                break;
            };
            if message_size > 100 * 1024 * 1024 {
                // 100MB limit
                warn!("Message too large: {message_size} bytes");
                break;
            }

            // Read message data
            let mut message_buf = vec![0u8; message_size];
            if let Err(e) = stream.read_exact(&mut message_buf).await {
                warn!("Failed to read message data: {e}");
                break;
            }

            // Parse request
            let request: DaemonRequest = match serde_json::from_slice(&message_buf) {
                Ok(req) => req,
                Err(e) => {
                    warn!("Failed to parse request: {e}");
                    let response = DaemonResponse::error("Invalid JSON request");
                    if let Err(e) = self.send_response(&mut stream, &response).await {
                        warn!("Failed to send error response: {e}");
                    }
                    continue;
                }
            };

            // Validate request
            if let Err(e) = request.validate() {
                warn!("Request validation failed: {e}");
                let response = DaemonResponse::error(&format!("Request validation failed: {e}"));
                if let Err(e) = self.send_response(&mut stream, &response).await {
                    warn!("Failed to send validation error response: {e}");
                }
                continue;
            }

            // Check rate limits
            if let Err(e) = self.resource_manager.record_request(&client_id).await {
                warn!("Rate limit exceeded for client {client_id}: {e}");
                let response = DaemonResponse::error(&format!("Rate limit exceeded: {e}"));
                if let Err(e) = self.send_response(&mut stream, &response).await {
                    warn!("Failed to send rate limit error response: {e}");
                }
                continue;
            }

            // Handle special commands that may require persistent connections
            if matches!(
                request.command.as_str(),
                "subscribe" | "get_events" | "get_subscriber_info"
            ) {
                // Clean up the connection from resource manager before transferring to persistent handler
                self.resource_manager
                    .unregister_connection(&client_id)
                    .await;

                if let Err(e) = self.handle_persistent_client(stream, request).await {
                    error!("Error in persistent client handler: {e}");
                }
                return Ok(());
            }

            // Handle record commands: if already recording, handle synchronously
            // (toggle stop returns immediately); otherwise ACK and record in background.
            if request.command.as_str() == "record" {
                if let Err(e) = self.handle_record_request(&mut stream, request).await {
                    warn!("Record request handling failed: {e}");
                    break;
                }
                continue;
            }

            // Handle regular commands with stream access for authentication
            let response = self.handle_command(request).await;
            if let Err(e) = self.send_response(&mut stream, &response).await {
                warn!("Failed to send response: {e}");
                break;
            }
        }

        // Clean up the connection
        self.resource_manager
            .unregister_connection(&client_id)
            .await;

        Ok(())
    }

    /// Handle a record request: if already recording, respond synchronously
    /// (stop/transcribing); otherwise ACK immediately and record in background.
    async fn handle_record_request(
        &self,
        stream: &mut UnixStream,
        request: DaemonRequest,
    ) -> Result<()> {
        let is_recording = *self.is_recording.read().await;
        if is_recording {
            let response = self.handle_command(request).await;
            self.send_response(stream, &response).await?;
        } else {
            let ack = DaemonResponse::success().with_message("Recording started".to_string());
            self.send_response(stream, &ack).await?;

            let daemon = self.clone();
            tokio::spawn(async move {
                info!("🎤 Background record task started");
                let response = daemon.handle_command(request).await;
                if response.status != "success" {
                    warn!(
                        "Record command failed: {}",
                        response.message.unwrap_or_default()
                    );
                }
            });
        }
        Ok(())
    }

    /// Handle persistent client connections (for subscriptions and events)
    ///
    /// # Errors
    ///
    /// Returns an error if reading from or writing to the stream fails.
    pub async fn handle_persistent_client(
        &self,
        mut stream: UnixStream,
        initial_request: DaemonRequest,
    ) -> Result<()> {
        // Handle initial request
        let initial_response = self.handle_command(initial_request).await;
        self.send_response(&mut stream, &initial_response).await?;

        // If it was a subscribe command and successful, enter persistent mode
        if initial_response.status == "success" && initial_response.client_id.is_some() {
            let Some(client_id) = initial_response.client_id.clone() else {
                return Ok(());
            };

            // Set up notification streaming
            if let Some(subscriber) = self.notification_manager.subscribers.get(&client_id) {
                let mut receiver = subscriber.sender.subscribe();
                drop(subscriber); // Release the lock

                loop {
                    tokio::select! {
                        // Receive notification events
                        event_result = receiver.recv() => {
                            match event_result {
                                Ok(event) => {
                                    let event_json = serde_json::to_vec(&event)?;
                                    let size = event_json.len() as u64;

                                    if stream.write_all(&size.to_be_bytes()).await.is_err() ||
                                       stream.write_all(&event_json).await.is_err() {
                                        break;
                                    }
                                }
                                Err(broadcast::error::RecvError::Closed) => {
                                    break;
                                }
                                Err(broadcast::error::RecvError::Lagged(_)) => {
                                    warn!("Client {client_id} lagged behind, some events may be lost");
                                }
                            }
                        }

                        // Handle additional requests from client
                        read_result = async {
                            let mut size_buf = [0u8; 8];
                            stream.read_exact(&mut size_buf).await.map_err(|e| anyhow::anyhow!(e))?;
                            let message_size = usize::try_from(u64::from_be_bytes(size_buf))
                                .map_err(|e| anyhow::anyhow!(e))?;
                            let mut message_buf = vec![0u8; message_size];
                            stream.read_exact(&mut message_buf).await.map_err(|e| anyhow::anyhow!(e))?;
                            serde_json::from_slice::<DaemonRequest>(&message_buf).map_err(|e| anyhow::anyhow!(e))
                        } => {
                            match read_result {
                                Ok(request) => {
                                    // Validate persistent client requests too
                                    if let Err(e) = request.validate() {
                                        warn!("Persistent client request validation failed: {e}");
                                        let response = DaemonResponse::error(&format!("Request validation failed: {e}"));
                                        if self.send_response(&mut stream, &response).await.is_err() {
                                            break;
                                        }
                                    } else {
                                        let response = self.handle_command(request).await;
                                        if self.send_response(&mut stream, &response).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                                Err(_) => break, // Client disconnected
                            }
                        }
                    }
                }

                // Clean up subscription
                self.notification_manager.unsubscribe(&client_id);

                // Stop audio monitoring if no audio_level subscribers remain
                self.ensure_audio_monitoring_stopped().await;
            }
        }

        Ok(())
    }

    /// Send response to client
    ///
    /// # Errors
    ///
    /// Returns an error if writing to the stream fails.
    pub async fn send_response(
        &self,
        stream: &mut UnixStream,
        response: &DaemonResponse,
    ) -> Result<()> {
        let response_data = serde_json::to_vec(response)?;
        let size = response_data.len() as u64;

        stream.write_all(&size.to_be_bytes()).await?;
        stream.write_all(&response_data).await?;

        Ok(())
    }

    /// Update client connection timestamp
    pub async fn update_client_connection(&self, client_id: String) {
        let connection = ClientConnection {
            last_seen: Utc::now(),
        };

        let mut connections = self.active_connections.write().await;
        connections.insert(client_id, connection);
    }

    /// Check if a client connection is still active (within timeout)
    pub async fn is_client_connection_active(&self, client_id: &str) -> bool {
        let connections = self.active_connections.read().await;
        if let Some(connection) = connections.get(client_id) {
            let now = Utc::now();
            let timeout = chrono::Duration::seconds(30); // 30 second timeout
            now.signed_duration_since(connection.last_seen) < timeout
        } else {
            false
        }
    }

    /// Clean up old connections
    pub async fn cleanup_old_connections(&self) {
        let mut connections = self.active_connections.write().await;
        let now = Utc::now();
        let timeout = chrono::Duration::seconds(30);

        connections
            .retain(|_, connection| now.signed_duration_since(connection.last_seen) < timeout);
    }

    /// Stop audio monitoring if no `audio_level` subscribers remain
    pub async fn ensure_audio_monitoring_stopped(&self) {
        // Check both notification subscribers and UDP clients
        let has_clients = self.has_audio_level_clients(&self.udp_streamer).await;

        if !has_clients {
            log::info!("No audio_level clients remaining, stopping audio monitoring");
            let mut handle_guard = self.audio_monitoring_handle.write().await;
            if let Some(handle) = handle_guard.take() {
                handle.abort();
                log::info!("Audio monitoring stopped");
            }
        }
    }

    /// Check if there are any clients that need audio level data (notification subscribers or UDP capability)
    #[allow(clippy::unused_async)]
    pub async fn has_audio_level_clients(
        &self,
        _udp_streamer: &std::sync::Arc<crate::audio::streamer::UdpAudioStreamer>,
    ) -> bool {
        // Check notification subscribers
        if self
            .notification_manager
            .has_subscribers_for_event("audio_level")
        {
            return true;
        }

        // If UDP streamer exists, we should monitor for potential clients
        // This ensures UDP clients (like COSMIC applet) get immediate data when they register
        true
    }

    /// Broadcast recording state change to all clients
    pub async fn broadcast_recording_state_change(&self, is_recording: bool) {
        // Broadcast recording state via UDP to applet
        if let Err(e) = &self
            .udp_streamer
            .broadcast_recording_state(
                is_recording,
                0, // daemon client ID
            )
            .await
        {
            warn!("Failed to broadcast recording state via UDP: {e}");
        }
    }
}
