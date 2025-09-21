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

/// Send a record command to the daemon and get transcription result
pub async fn send_record_command(socket_path: PathBuf) -> Result<String, String> {
    let result =
        super_stt_shared::daemon::client::send_record_command(socket_path, get_client_id()).await?;

    // Handle the specific formatting the app expects
    if result.trim().is_empty() {
        Ok("No speech detected".to_string())
    } else {
        Ok(result)
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

/// Subscribe to daemon events (particularly device switching events)
#[allow(dead_code)]
pub async fn subscribe_to_daemon_events(socket_path: PathBuf) -> Result<String, String> {
    super_stt_shared::daemon::client::send_daemon_command(
        socket_path,
        "subscribe",
        Some(serde_json::json!({
            "event_types": ["daemon_status_changed"],
            "client_info": {}
        })),
        get_client_id(),
    )
    .await
}

/// Get recent daemon events
#[allow(dead_code)]
pub async fn get_daemon_events(
    socket_path: PathBuf,
    since_timestamp: Option<String>,
) -> Result<Vec<super_stt_shared::models::protocol::NotificationEvent>, String> {
    super_stt_shared::daemon::client::get_daemon_events(
        socket_path,
        get_client_id(),
        since_timestamp,
        Some(vec![
            "daemon_status_changed".to_string(),
            "download_progress".to_string(),
        ]),
        Some(50),
    )
    .await
}
