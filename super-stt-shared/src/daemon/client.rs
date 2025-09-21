// SPDX-License-Identifier: GPL-3.0-only
//! Shared daemon client functionality for Super STT applications

use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::models::protocol::{DaemonRequest, DaemonResponse, DownloadProgress};
use crate::stt_model::STTModel;

/// Basic daemon connection utility with improved error handling
async fn connect_to_daemon(socket_path: &PathBuf) -> Result<UnixStream, String> {
    UnixStream::connect(socket_path)
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                "Daemon not running. Start the daemon first.".to_string()
            }
            _ => format!("Connection failed: {e}"),
        })
}

/// Send a request to the daemon and get the response using size-prefixed protocol
/// Send a request to the daemon and read back the JSON response.
///
/// # Errors
///
/// Returns an error when connecting, serializing, writing, reading, or parsing fails,
/// or if the response length does not fit in `usize` on this platform.
async fn send_daemon_request(
    socket_path: &PathBuf,
    request: DaemonRequest,
) -> Result<DaemonResponse, String> {
    let mut stream = connect_to_daemon(socket_path).await?;

    // Serialize request and get size
    let request_data =
        serde_json::to_vec(&request).map_err(|e| format!("Failed to serialize request: {e}"))?;

    // Send size header (8 bytes, big-endian)
    let size = request_data.len() as u64;
    stream
        .write_all(&size.to_be_bytes())
        .await
        .map_err(|e| format!("Failed to write size: {e}"))?;

    // Send request data
    stream
        .write_all(&request_data)
        .await
        .map_err(|e| format!("Failed to write request: {e}"))?;

    // Read size header from response
    let mut size_buf = [0u8; 8];
    stream
        .read_exact(&mut size_buf)
        .await
        .map_err(|e| format!("Failed to read response size: {e}"))?;

    // Read exact response size
    let response_size = u64::from_be_bytes(size_buf);
    let response_len = usize::try_from(response_size)
        .map_err(|_| "Response too large for this platform".to_string())?;
    let mut response_buf = vec![0u8; response_len];
    stream
        .read_exact(&mut response_buf)
        .await
        .map_err(|e| format!("Failed to read response: {e}"))?;

    // Parse response
    serde_json::from_slice(&response_buf).map_err(|e| format!("Failed to parse response: {e}"))
}

/// Create a basic daemon request with client identification
#[must_use]
pub fn create_daemon_request(command: &str, client_id: &str) -> DaemonRequest {
    DaemonRequest {
        command: command.to_string(),
        data: None,
        client_id: Some(client_id.to_string()),
        language: None,
        audio_data: None,
        sample_rate: None,
        event_types: None,
        client_info: None,
        since_timestamp: None,
        limit: None,
        event_type: None,
        enabled: None,
    }
}

/// Ping daemon to check if it's running and responsive
///
/// # Errors
///
/// Returns an error if the ping request cannot be delivered or returns an error status.
pub async fn ping_daemon(socket_path: PathBuf, client_id: &str) -> Result<String, String> {
    let request = create_daemon_request("ping", client_id);
    let response = send_daemon_request(&socket_path, request).await?;

    if response.status == "success" {
        Ok(response
            .message
            .unwrap_or_else(|| "Daemon is running".to_string()))
    } else {
        Err(response
            .message
            .unwrap_or_else(|| "Ping failed".to_string()))
    }
}

/// Extended ping response with connection status information
pub struct PingResponse {
    pub message: String,
    pub connection_active: bool,
}

/// Ping daemon and get extended connection status information
///
/// # Errors
///
/// Returns an error if the ping request cannot be delivered or returns an error status.
pub async fn ping_daemon_with_status(
    socket_path: PathBuf,
    client_id: &str,
) -> Result<PingResponse, String> {
    let request = create_daemon_request("ping", client_id);
    let response = send_daemon_request(&socket_path, request).await?;

    if response.status == "success" {
        let message = response
            .message
            .unwrap_or_else(|| "Daemon is running".to_string());
        let connection_active = response.connection_active.unwrap_or(true); // Default to true for backward compatibility

        Ok(PingResponse {
            message,
            connection_active,
        })
    } else {
        Err(response
            .message
            .unwrap_or_else(|| "Ping failed".to_string()))
    }
}

/// Send a record command to the daemon
///
/// # Errors
///
/// Returns an error if the request fails or the daemon responds with an error.
pub async fn send_record_command(socket_path: PathBuf, client_id: &str) -> Result<String, String> {
    let request = create_daemon_request("record", client_id);
    let response = send_daemon_request(&socket_path, request).await?;

    if response.status == "success" {
        Ok(response
            .transcription
            .or(response.message)
            .unwrap_or_else(|| "No transcription received".to_string()))
    } else {
        Err(response
            .message
            .unwrap_or_else(|| "Recording failed".to_string()))
    }
}

/// Get current daemon configuration
///
/// # Errors
///
/// Returns an error if the request fails or the daemon responds without a config.
pub async fn fetch_daemon_config(
    socket_path: PathBuf,
    client_id: &str,
) -> Result<serde_json::Value, String> {
    let request = create_daemon_request("get_config", client_id);
    let response = send_daemon_request(&socket_path, request).await?;

    if response.status == "success" {
        response
            .daemon_config
            .ok_or_else(|| "No daemon config in response".to_string())
    } else {
        Err(response
            .message
            .unwrap_or_else(|| "Failed to get daemon config".to_string()))
    }
}

/// Set audio theme on daemon
///
/// # Errors
///
/// Returns an error if the request fails or the daemon responds with an error.
pub async fn set_audio_theme(
    socket_path: PathBuf,
    theme: &str,
    client_id: &str,
) -> Result<String, String> {
    let mut request = create_daemon_request("set_audio_theme", client_id);
    request.data = Some(serde_json::json!({"theme": theme}));

    let response = send_daemon_request(&socket_path, request).await?;

    if response.status == "success" {
        Ok(response
            .message
            .unwrap_or_else(|| "Audio theme set successfully".to_string()))
    } else {
        Err(response
            .message
            .unwrap_or_else(|| "Failed to set audio theme".to_string()))
    }
}

/// Test audio theme on daemon
///
/// # Errors
///
/// Returns an error if the request fails or playback cannot be verified.
pub async fn test_audio_theme(socket_path: PathBuf, client_id: &str) -> Result<String, String> {
    let request = create_daemon_request("test_audio_theme", client_id);
    let response = send_daemon_request(&socket_path, request).await?;

    if response.status == "success" {
        Ok(response
            .message
            .unwrap_or_else(|| "Theme tested successfully".to_string()))
    } else {
        Err(response
            .message
            .unwrap_or_else(|| "Failed to test theme".to_string()))
    }
}

/// Set and test audio theme in one operation
///
/// # Errors
///
/// Returns an error if setting the theme or the test step fails.
pub async fn set_and_test_audio_theme(
    socket_path: PathBuf,
    theme: &str,
    client_id: &str,
) -> Result<String, String> {
    set_audio_theme(socket_path.clone(), theme, client_id).await?;
    test_audio_theme(socket_path, client_id).await
}

/// Send a generic command to the daemon with optional data
///
/// # Errors
///
/// Returns an error if the request fails or the daemon responds with an error.
pub async fn send_daemon_command(
    socket_path: PathBuf,
    command: &str,
    data: Option<serde_json::Value>,
    client_id: &str,
) -> Result<String, String> {
    let mut request = create_daemon_request(command, client_id);
    request.data = data;

    let response = send_daemon_request(&socket_path, request).await?;

    if response.status == "success" {
        Ok(response.message.unwrap_or_else(|| "Success".to_string()))
    } else {
        Err(response
            .message
            .unwrap_or_else(|| "Unknown error".to_string()))
    }
}

/// Test daemon connection
///
/// # Errors
///
/// Returns an error if the ping request fails.
pub async fn test_daemon_connection(socket_path: PathBuf, client_id: &str) -> Result<(), String> {
    ping_daemon(socket_path, client_id).await.map(|_| ())
}

/// Get current loaded model from daemon
///
/// # Errors
///
/// Returns an error if the request fails or the daemon doesn't return model info.
pub async fn get_current_model(socket_path: PathBuf, client_id: &str) -> Result<STTModel, String> {
    let request = create_daemon_request("get_model", client_id);
    let response = send_daemon_request(&socket_path, request).await?;

    if response.status == "success" {
        Ok(response.current_model.unwrap_or_default())
    } else {
        Err(response
            .message
            .unwrap_or_else(|| "Failed to get current model".to_string()))
    }
}

/// Set/switch to a different model
///
/// # Errors
///
/// Returns an error if the model switch request fails.
pub async fn set_model(
    socket_path: PathBuf,
    model: STTModel,
    client_id: &str,
) -> Result<String, String> {
    let data = serde_json::json!({ "model": model.to_string() });
    send_daemon_command(socket_path, "set_model", Some(data), client_id).await
}

/// List all available models from daemon
///
/// # Errors
///
/// Returns an error if the request fails or the daemon doesn't return model list.
pub async fn list_available_models(
    socket_path: PathBuf,
    client_id: &str,
) -> Result<Vec<STTModel>, String> {
    let request = create_daemon_request("list_models", client_id);
    let response = send_daemon_request(&socket_path, request).await?;

    if response.status == "success" {
        Ok(response.available_models.unwrap_or_default())
    } else {
        Err(response
            .message
            .unwrap_or_else(|| "Failed to get available models".to_string()))
    }
}

/// Cancel any ongoing download
///
/// # Errors
///
/// Returns an error if the cancel request fails.
pub async fn cancel_download(socket_path: PathBuf, client_id: &str) -> Result<String, String> {
    send_daemon_command(socket_path, "cancel_download", None, client_id).await
}

/// Get current download status
///
/// # Errors
///
/// Returns an error if the request fails.
pub async fn get_download_status(
    socket_path: PathBuf,
    client_id: &str,
) -> Result<Option<DownloadProgress>, String> {
    let request = create_daemon_request("get_download_status", client_id);
    let response = send_daemon_request(&socket_path, request).await?;

    if response.status == "success" {
        Ok(response.download_progress)
    } else {
        Err(response
            .message
            .unwrap_or_else(|| "Failed to get download status".to_string()))
    }
}

/// Get current device and available devices from daemon
///
/// # Errors
///
/// Returns an error if the request fails.
pub async fn get_current_device(
    socket_path: PathBuf,
    client_id: &str,
) -> Result<(String, Vec<String>), String> {
    let request = create_daemon_request("get_device", client_id);
    let response = send_daemon_request(&socket_path, request).await?;

    if response.status == "success" {
        let device = response.device.unwrap_or_else(|| "unknown".to_string());
        let available_devices = response
            .available_devices
            .unwrap_or_else(|| vec!["cpu".to_string()]);
        Ok((device, available_devices))
    } else {
        Err(response
            .message
            .unwrap_or_else(|| "Failed to get current device".to_string()))
    }
}

/// Set device on daemon
///
/// # Errors
///
/// Returns an error if the request fails.
pub async fn set_device(
    socket_path: PathBuf,
    device: String,
    client_id: &str,
) -> Result<(), String> {
    let data = serde_json::json!({"device": device});
    send_daemon_command(socket_path, "set_device", Some(data), client_id)
        .await
        .map(|_| ())
}

/// List available audio themes from daemon
///
/// # Errors
///
/// Returns an error if the request fails or the daemon doesn't return audio theme list.
pub async fn list_available_audio_themes(
    socket_path: PathBuf,
    client_id: &str,
) -> Result<Vec<String>, String> {
    let request = create_daemon_request("list_audio_themes", client_id);
    let response = send_daemon_request(&socket_path, request).await?;

    if response.status == "success" {
        // Convert available_audio_themes to strings if it exists
        let themes = response
            .available_audio_themes
            .unwrap_or_default()
            .into_iter()
            .map(|theme| theme.to_string())
            .collect();
        Ok(themes)
    } else {
        Err(response
            .message
            .unwrap_or_else(|| "Unknown error".to_string()))
    }
}

/// Set preview typing enabled/disabled on daemon
///
/// # Errors
///
/// Returns an error if the request fails.
pub async fn set_preview_typing(
    socket_path: PathBuf,
    enabled: bool,
    client_id: &str,
) -> Result<(), String> {
    let mut request = create_daemon_request("set_preview_typing", client_id);
    request.enabled = Some(enabled);

    let response = send_daemon_request(&socket_path, request).await?;

    if response.status == "success" {
        Ok(())
    } else {
        Err(response
            .message
            .unwrap_or_else(|| "Failed to set preview typing".to_string()))
    }
}

/// Get current preview typing setting from daemon
///
/// # Errors
///
/// Returns an error if the request fails.
pub async fn get_preview_typing(socket_path: PathBuf, client_id: &str) -> Result<bool, String> {
    let request = create_daemon_request("get_preview_typing", client_id);
    let response = send_daemon_request(&socket_path, request).await?;

    if response.status == "success" {
        Ok(response.preview_typing_enabled.unwrap_or(false))
    } else {
        Err(response
            .message
            .unwrap_or_else(|| "Failed to get preview typing setting".to_string()))
    }
}

/// Get recent daemon events
///
/// # Errors
///
/// Returns an error if the request fails.
pub async fn get_daemon_events(
    socket_path: PathBuf,
    client_id: &str,
    since_timestamp: Option<String>,
    event_types: Option<Vec<String>>,
    limit: Option<u32>,
) -> Result<Vec<crate::models::protocol::NotificationEvent>, String> {
    let mut request = create_daemon_request("get_events", client_id);
    request.data = Some(serde_json::json!({
        "since_timestamp": since_timestamp,
        "event_types": event_types.unwrap_or_else(|| vec!["daemon_status_changed".to_string()]),
        "limit": limit.unwrap_or(50)
    }));

    let response = send_daemon_request(&socket_path, request).await?;

    if response.status == "success" {
        Ok(response.events.unwrap_or_default())
    } else {
        Err(response
            .message
            .unwrap_or_else(|| "Failed to get daemon events".to_string()))
    }
}
