// SPDX-License-Identifier: GPL-3.0-only
use std::path::PathBuf;
use std::sync::OnceLock;
use super_stt_shared::stt_model::STTModel;

use crate::state::AudioTheme;

// Generate a unique client ID for this app instance
static CLIENT_ID: OnceLock<String> = OnceLock::new();

fn get_client_id() -> &'static str {
    CLIENT_ID
        .get_or_init(|| super_stt_shared::validation::generate_secure_client_id("super-stt-app"))
}

/// Result type for streaming record responses.
pub enum RecordEvent {
    /// Intermediate preview text during recording.
    Preview(String),
    /// Final transcription result.
    Final(Result<String, String>),
}

/// Send a record command to the daemon and stream results.
/// Yields `RecordEvent::Preview` for intermediate previews and
/// `RecordEvent::Final` when the transcription is complete.
pub fn record_command_stream(
    socket_path: PathBuf,
) -> impl futures_util::Stream<Item = RecordEvent> + Send + 'static {
    cosmic::iced::stream::channel(
        32,
        move |mut channel: cosmic::iced::futures::channel::mpsc::Sender<RecordEvent>| async move {
            use futures_util::SinkExt;
            use super_stt_shared::models::protocol::DaemonResponse;
            use tokio::io::{AsyncReadExt, AsyncWriteExt};

            let result: Result<(), String> = async {
                let mut stream = tokio::net::UnixStream::connect(&socket_path)
                    .await
                    .map_err(|e| format!("Failed to connect: {e}"))?;

                let mut request = super_stt_shared::daemon::client::create_daemon_request(
                    "record",
                    get_client_id(),
                );
                request.data = Some(serde_json::json!({
                    "write_mode": false,
                    "stop_mode": "manual-only",
                    "wait": true,
                }));

                let data =
                    serde_json::to_vec(&request).map_err(|e| format!("Serialize failed: {e}"))?;
                let size = data.len() as u64;
                stream
                    .write_all(&size.to_be_bytes())
                    .await
                    .map_err(|e| format!("Write failed: {e}"))?;
                stream
                    .write_all(&data)
                    .await
                    .map_err(|e| format!("Write failed: {e}"))?;

                loop {
                    let mut size_buf = [0u8; 8];
                    if stream.read_exact(&mut size_buf).await.is_err() {
                        break;
                    }
                    let resp_size = u64::from_be_bytes(size_buf);
                    let resp_len =
                        usize::try_from(resp_size).map_err(|_| "Response too large".to_string())?;
                    let mut resp_buf = vec![0u8; resp_len];
                    stream
                        .read_exact(&mut resp_buf)
                        .await
                        .map_err(|e| format!("Read failed: {e}"))?;
                    let response: DaemonResponse = serde_json::from_slice(&resp_buf)
                        .map_err(|e| format!("Parse failed: {e}"))?;

                    if let Some(preview) = response.preview_text {
                        let _ = channel.send(RecordEvent::Preview(preview)).await;
                        continue;
                    }

                    // Final response
                    if response.status == "success" {
                        let text = response
                            .transcription
                            .or(response.message)
                            .unwrap_or_default();
                        let result = if text.trim().is_empty() {
                            "No speech detected".to_string()
                        } else {
                            text
                        };
                        let _ = channel.send(RecordEvent::Final(Ok(result))).await;
                    } else {
                        let err = response
                            .message
                            .unwrap_or_else(|| "Recording failed".to_string());
                        let _ = channel.send(RecordEvent::Final(Err(err))).await;
                    }
                    break;
                }
                Ok(())
            }
            .await;

            if let Err(e) = result {
                let _ = channel.send(RecordEvent::Final(Err(e))).await;
            }
        },
    )
}

/// Send a stop signal to a running recording.
pub async fn stop_record_command(socket_path: PathBuf) -> Result<(), String> {
    let mut request =
        super_stt_shared::daemon::client::create_daemon_request("record", get_client_id());
    request.data = Some(serde_json::json!({
        "write_mode": false,
        "stop_mode": "manual-only",
    }));

    let response =
        super_stt_shared::daemon::client::send_daemon_request_pub(&socket_path, request).await?;

    if response.status == "success" {
        Ok(())
    } else {
        Err(response
            .message
            .unwrap_or_else(|| "Stop failed".to_string()))
    }
}

/// Test daemon connection
pub async fn test_daemon_connection(socket_path: PathBuf) -> Result<(), String> {
    super_stt_shared::daemon::client::test_daemon_connection(socket_path, get_client_id()).await
}

/// Load available audio themes from daemon with fallback
pub async fn load_audio_themes(socket_path: PathBuf) -> Vec<AudioTheme> {
    // Try to get available themes from daemon
    if let Ok(themes) = list_available_audio_themes(socket_path.clone()).await {
        return themes;
    }

    // Fallback to all available themes if daemon is unavailable
    AudioTheme::all_themes()
}

/// List available audio themes from daemon
pub async fn list_available_audio_themes(socket_path: PathBuf) -> Result<Vec<AudioTheme>, String> {
    let theme_strings =
        super_stt_shared::daemon::client::list_available_audio_themes(socket_path, get_client_id())
            .await?;

    // Convert strings back to AudioTheme enum
    let themes = theme_strings
        .into_iter()
        .filter_map(|theme_str| theme_str.parse::<AudioTheme>().ok())
        .collect();

    Ok(themes)
}

/// Set audio theme without playing a test sound
pub async fn set_audio_theme(socket_path: PathBuf, theme: AudioTheme) -> Result<String, String> {
    super_stt_shared::daemon::client::set_audio_theme(
        socket_path,
        &theme.to_string().to_lowercase(),
        get_client_id(),
    )
    .await
}

/// Set and test audio theme - convenience function
pub async fn set_and_test_audio_theme(
    socket_path: PathBuf,
    theme: AudioTheme,
) -> Result<String, String> {
    super_stt_shared::daemon::client::set_and_test_audio_theme(
        socket_path,
        &theme.to_string().to_lowercase(),
        get_client_id(),
    )
    .await
}

/// Ping daemon to check connectivity
pub async fn ping_daemon(socket_path: PathBuf) -> Result<String, String> {
    super_stt_shared::daemon::client::ping_daemon(socket_path, get_client_id()).await
}

/// Get current loaded model from daemon
pub async fn get_current_model(socket_path: PathBuf) -> Result<STTModel, String> {
    super_stt_shared::daemon::client::get_current_model(socket_path, get_client_id()).await
}

/// Set/switch to a different model
pub async fn set_model(socket_path: PathBuf, model: STTModel) -> Result<String, String> {
    super_stt_shared::daemon::client::set_model(socket_path, model, get_client_id()).await
}

/// List all available models from daemon
pub async fn list_available_models(socket_path: PathBuf) -> Result<Vec<STTModel>, String> {
    super_stt_shared::daemon::client::list_available_models(socket_path, get_client_id()).await
}

/// Cancel any ongoing download
pub async fn cancel_download(socket_path: PathBuf) -> Result<String, String> {
    super_stt_shared::daemon::client::cancel_download(socket_path, get_client_id()).await
}

/// Get current download status
pub async fn get_download_status(
    socket_path: PathBuf,
) -> Result<Option<super_stt_shared::models::protocol::DownloadProgress>, String> {
    super_stt_shared::daemon::client::get_download_status(socket_path, get_client_id()).await
}

/// Get current device and available devices from daemon
pub async fn get_current_device(socket_path: PathBuf) -> Result<(String, Vec<String>), String> {
    super_stt_shared::daemon::client::get_current_device(socket_path, get_client_id()).await
}

/// Set device on daemon
pub async fn set_device(socket_path: PathBuf, device: String) -> Result<(), String> {
    super_stt_shared::daemon::client::set_device(socket_path, device, get_client_id()).await
}

/// Get current daemon configuration
pub async fn fetch_daemon_config(socket_path: PathBuf) -> Result<serde_json::Value, String> {
    super_stt_shared::daemon::client::fetch_daemon_config(socket_path, get_client_id()).await
}

/// Set preview typing enabled/disabled on daemon
pub async fn set_preview_typing(socket_path: PathBuf, enabled: bool) -> Result<(), String> {
    super_stt_shared::daemon::client::set_preview_typing(socket_path, enabled, get_client_id())
        .await
}

/// Get current preview typing setting from daemon
pub async fn get_preview_typing(socket_path: PathBuf) -> Result<bool, String> {
    super_stt_shared::daemon::client::get_preview_typing(socket_path, get_client_id()).await
}

/// Set recording stop mode on daemon
pub async fn set_recording_stop_mode(socket_path: PathBuf, mode: String) -> Result<(), String> {
    super_stt_shared::daemon::client::set_recording_stop_mode(socket_path, &mode, get_client_id())
        .await
}

/// Get current recording stop mode from daemon
pub async fn get_recording_stop_mode(socket_path: PathBuf) -> Result<String, String> {
    super_stt_shared::daemon::client::get_recording_stop_mode(socket_path, get_client_id()).await
}

/// Set write method on daemon
pub async fn set_write_method(socket_path: PathBuf, method: String) -> Result<(), String> {
    super_stt_shared::daemon::client::set_write_method(socket_path, &method, get_client_id()).await
}

/// Get current write method from daemon
pub async fn get_write_method(socket_path: PathBuf) -> Result<String, String> {
    super_stt_shared::daemon::client::get_write_method(socket_path, get_client_id()).await
}

/// Set master volume on daemon
pub async fn set_volume(socket_path: PathBuf, volume: u8) -> Result<(), String> {
    super_stt_shared::daemon::client::set_volume(socket_path, volume, get_client_id()).await
}
