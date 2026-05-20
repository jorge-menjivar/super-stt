// SPDX-License-Identifier: GPL-3.0-only
//! Daemon-facing operations for the settings app.
//!
//! All commands here go through the new HTTP protocol. Each call uses
//! `super_stt_shared::daemon::http_client` with a session token cached
//! in the system keyring (`session::with_token`). The token is minted
//! on first call via the daemon's libcosmic consent popup; subsequent
//! calls reuse the cached token. On `invalid_session` the cache is
//! dropped and re-auth runs once.
//!
//! Recording streaming uses the SSE-based `/transcribe` endpoint with
//! `event: preview`, `event: done`, and `event: error` frames.
//! Closing the stream mid-recording triggers a server-side stop.

use crate::state::AudioTheme;
use super_stt_shared::daemon::http_client;
use super_stt_shared::daemon::session::{self, AppId};
use super_stt_shared::validation::get_http_socket_path;

const SETTINGS_SCOPE: &str = "settings";
const APP_NAME: &str = "Super STT Settings App";
const APP_ID_NAME: AppId = AppId("super-stt-app");

/// Run an HTTP-protocol operation with the cached settings-scope token.
/// On `invalid_session` the cache is invalidated and the operation
/// retries once with a fresh consent flow.
async fn with_settings_token<F, Fut, T>(op: F) -> Result<T, String>
where
    F: Fn(std::path::PathBuf, String) -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let socket = get_http_socket_path();
    let socket_for_op = socket.clone();
    session::with_token(
        socket,
        APP_ID_NAME,
        APP_NAME,
        SETTINGS_SCOPE,
        move |token| op(socket_for_op.clone(), token),
    )
    .await
}

// =============================================================================
// Recording — HTTP/SSE via /transcribe
// =============================================================================

/// Result type for streaming record responses.
pub enum RecordEvent {
    /// Intermediate preview text during recording. Snapshot semantics:
    /// the text replaces (not appends to) any previously-displayed
    /// preview.
    Preview(String),
    /// Final transcription result.
    Final(Result<String, String>),
}

/// Start a recording and stream `RecordEvent`s as the daemon emits SSE
/// events on `POST /transcribe`. Closing the returned stream early
/// signals the daemon to stop the recording.
pub fn record_command_stream() -> impl futures_util::Stream<Item = RecordEvent> + Send + 'static {
    cosmic::iced::stream::channel(
        32,
        move |mut channel: cosmic::iced::futures::channel::mpsc::Sender<RecordEvent>| async move {
            use futures_util::{SinkExt, StreamExt};
            use super_stt_shared::daemon::http_client::{TranscribeEvent, TranscribeOptions};

            let result: Result<(), String> = async {
                let socket = get_http_socket_path();
                let token =
                    session::obtain(socket.clone(), APP_ID_NAME, APP_NAME, SETTINGS_SCOPE).await?;

                let opts = TranscribeOptions {
                    wait: true,
                    write_mode: false,
                    stop_mode: Some("manual-only".to_string()),
                };
                let mut stream =
                    Box::pin(http_client::transcribe_stream(socket, &token, opts).await?);

                while let Some(event) = stream.next().await {
                    match event {
                        TranscribeEvent::Preview(text) => {
                            let _ = channel.send(RecordEvent::Preview(text)).await;
                        }
                        TranscribeEvent::Done(text) => {
                            let result = if text.trim().is_empty() {
                                "No speech detected".to_string()
                            } else {
                                text
                            };
                            let _ = channel.send(RecordEvent::Final(Ok(result))).await;
                            return Ok(());
                        }
                        TranscribeEvent::Error(msg) => {
                            let _ = channel.send(RecordEvent::Final(Err(msg))).await;
                            return Ok(());
                        }
                    }
                }
                let _ = channel
                    .send(RecordEvent::Final(Err(
                        "transcribe stream ended unexpectedly".to_string(),
                    )))
                    .await;
                Ok(())
            }
            .await;

            if let Err(e) = result {
                let _ = channel.send(RecordEvent::Final(Err(e))).await;
            }
        },
    )
}

/// Send a stop signal to a running recording (HTTP `POST /transcribe/stop`).
pub async fn stop_record_command() -> Result<(), String> {
    with_settings_token(|socket, token| async move {
        let resp = http_client::transcribe_stop(socket, &token).await?;
        if resp.status != "success" {
            return Err(resp.message.unwrap_or_else(|| "Stop failed".to_string()));
        }
        Ok(())
    })
    .await
}

// =============================================================================
// Settings calls — daemon is reached via the new /endpoint surface.
// =============================================================================

/// Test daemon connection (HTTP `/ping`).
pub async fn test_daemon_connection() -> Result<(), String> {
    with_settings_token(|socket, token| async move {
        http_client::ping(socket, &token).await.map(|_| ())
    })
    .await
}

/// Ping daemon to check connectivity (HTTP `/ping`).
pub async fn ping_daemon() -> Result<String, String> {
    with_settings_token(|socket, token| async move { http_client::ping(socket, &token).await })
        .await
}

/// Load available audio themes from daemon with fallback.
pub async fn load_audio_themes() -> Vec<AudioTheme> {
    list_available_audio_themes()
        .await
        .unwrap_or_else(|_| AudioTheme::all_themes())
}

/// List available audio themes from daemon (HTTP `/audio_themes`).
pub async fn list_available_audio_themes() -> Result<Vec<AudioTheme>, String> {
    with_settings_token(|socket, token| async move {
        let resp = http_client::list_audio_themes(socket, &token).await?;
        if resp.status != "success" {
            return Err(resp
                .message
                .unwrap_or_else(|| "list_themes failed".to_string()));
        }
        let themes = resp
            .available_audio_themes
            .unwrap_or_default()
            .into_iter()
            .map(|t| t.to_string())
            .filter_map(|s| s.parse::<AudioTheme>().ok())
            .collect();
        Ok(themes)
    })
    .await
}

/// Set audio theme without playing a test sound (HTTP `POST /audio_theme`).
pub async fn set_audio_theme(theme: AudioTheme) -> Result<String, String> {
    let theme_str = theme.to_string().to_lowercase();
    with_settings_token(move |socket, token| {
        let theme_str = theme_str.clone();
        async move {
            let resp = http_client::set_audio_theme(socket, &token, &theme_str).await?;
            if resp.status != "success" {
                return Err(resp
                    .message
                    .unwrap_or_else(|| "set_theme failed".to_string()));
            }
            Ok(resp.message.unwrap_or_default())
        }
    })
    .await
}

/// Set and test audio theme — convenience function (`POST /audio_theme` + `POST /audio_theme/test`).
pub async fn set_and_test_audio_theme(theme: AudioTheme) -> Result<String, String> {
    set_audio_theme(theme).await?;
    with_settings_token(|socket, token| async move {
        let resp = http_client::test_audio_theme(socket, &token).await?;
        Ok(resp.message.unwrap_or_default())
    })
    .await
}

/// Get current loaded model from daemon as `(name, provider, source)`
/// (HTTP `GET /active_model`).
pub async fn get_current_model() -> Result<
    (
        String,
        super_stt_shared::models::provider::Provider,
        super_stt_shared::models::registry::SourceKind,
    ),
    String,
> {
    with_settings_token(|socket, token| async move {
        let status = http_client::get_active_model(socket, &token).await?;
        let current = status.active_model.current;
        let model = current.model.ok_or("missing active_model.current.model")?;
        let provider_str = current
            .provider
            .ok_or("missing active_model.current.provider")?;
        let provider: super_stt_shared::models::provider::Provider = provider_str
            .parse()
            .map_err(|e| format!("invalid provider {provider_str:?}: {e}"))?;
        let source_str = current
            .source
            .ok_or("missing active_model.current.source")?;
        let source: super_stt_shared::models::registry::SourceKind = source_str
            .parse()
            .map_err(|e| format!("invalid source {source_str:?}: {e}"))?;
        Ok((model, provider, source))
    })
    .await
}

/// Set/switch to a different model (HTTP `POST /active_model`).
pub async fn set_model(
    model: String,
    provider: super_stt_shared::models::provider::Provider,
    source: super_stt_shared::models::registry::SourceKind,
) -> Result<String, String> {
    let provider_str = provider.to_string();
    let source_str = source.to_string();
    with_settings_token(move |socket, token| {
        let model = model.clone();
        let provider_str = provider_str.clone();
        let source_str = source_str.clone();
        async move {
            let resp = http_client::set_active_model(
                socket,
                &token,
                &model,
                &provider_str,
                Some(&source_str),
            )
            .await?;
            if resp.status != "success" {
                return Err(resp
                    .message
                    .unwrap_or_else(|| "set_model failed".to_string()));
            }
            Ok(resp.message.unwrap_or_default())
        }
    })
    .await
}

/// List all available models from daemon (HTTP `GET /models`).
pub async fn list_available_models() -> Result<
    Vec<(
        String,
        super_stt_shared::models::provider::Provider,
        super_stt_shared::models::registry::SourceKind,
    )>,
    String,
> {
    with_settings_token(|socket, token| async move {
        let resp = http_client::list_models(socket, &token).await?;
        if resp.status != "success" {
            return Err(resp
                .message
                .unwrap_or_else(|| "list_models failed".to_string()));
        }
        Ok(resp.available_models.unwrap_or_default())
    })
    .await
}

/// Cancel any ongoing model-switch download (HTTP `POST /active_model/cancel`).
pub async fn cancel_download() -> Result<String, String> {
    with_settings_token(|socket, token| async move {
        let resp = http_client::cancel_set_active_model(socket, &token).await?;
        Ok(resp.message.unwrap_or_default())
    })
    .await
}

/// Get current download status. Composed from `/active_model`'s
/// `switch.download` sub-object.
pub async fn get_download_status()
-> Result<Option<super_stt_shared::models::protocol::DownloadProgress>, String> {
    with_settings_token(|socket, token| async move {
        let status = http_client::get_active_model(socket, &token).await?;
        let Some(switch) = status.active_model.switch else {
            return Ok(None);
        };
        let Some(download) = switch.download else {
            return Ok(None);
        };
        let model_name = switch
            .target
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let progress = super_stt_shared::models::protocol::DownloadProgress {
            model_name,
            current_file: download.current_file,
            file_index: download.file_index,
            total_files: download.total_files,
            bytes_downloaded: download.bytes_downloaded,
            total_bytes: download.total_bytes,
            percentage: download.percentage,
            status: switch.phase,
            started_at: switch.started_at.unwrap_or_default(),
            eta_seconds: download.eta_seconds,
        };
        Ok(Some(progress))
    })
    .await
}

/// Get current device + GPU memory info (HTTP `GET /active_device`).
pub async fn get_current_device() -> Result<
    (
        String,
        Vec<String>,
        super_stt_shared::daemon::client::GpuMemoryInfo,
    ),
    String,
> {
    with_settings_token(|socket, token| async move {
        let resp = http_client::get_active_device(socket, &token).await?;
        if resp.status != "success" {
            return Err(resp
                .message
                .unwrap_or_else(|| "get_device failed".to_string()));
        }
        let device = resp.device.unwrap_or_else(|| "unknown".to_string());
        let available_devices = resp
            .available_devices
            .unwrap_or_else(|| vec!["cpu".to_string()]);
        let gpu_memory = match (resp.gpu_free_memory, resp.gpu_total_memory) {
            (Some(free), Some(total)) => Some((free, total)),
            _ => None,
        };
        Ok((device, available_devices, gpu_memory))
    })
    .await
}

/// Set device on daemon (HTTP `POST /active_device`).
pub async fn set_device(device: String) -> Result<(), String> {
    with_settings_token(move |socket, token| {
        let device = device.clone();
        async move {
            let resp = http_client::set_active_device(socket, &token, &device).await?;
            if resp.status != "success" {
                return Err(resp
                    .message
                    .unwrap_or_else(|| "set_device failed".to_string()));
            }
            Ok(())
        }
    })
    .await
}

/// Read currently-configured audio cue theme (HTTP `GET /audio_theme`).
pub async fn get_current_audio_theme() -> Result<AudioTheme, String> {
    with_settings_token(|socket, token| async move {
        let resp = http_client::get_audio_theme(socket, &token).await?;
        if resp.status != "success" {
            return Err(resp
                .message
                .unwrap_or_else(|| "get_audio_theme failed".to_string()));
        }
        let theme_str = resp.audio_theme.unwrap_or_default();
        Ok(theme_str.parse::<AudioTheme>().unwrap_or_default())
    })
    .await
}

/// Read current cue volume (HTTP `GET /volume`).
pub async fn get_volume() -> Result<u8, String> {
    with_settings_token(|socket, token| async move {
        let resp = http_client::get_volume(socket, &token).await?;
        if resp.status != "success" {
            return Err(resp
                .message
                .unwrap_or_else(|| "get_volume failed".to_string()));
        }
        // The legacy daemon returns volume in the `message` field as
        // text ("Volume is 75"). Parse it out.
        let msg = resp.message.unwrap_or_default();
        let vol = msg
            .rsplit(' ')
            .next()
            .and_then(|s| s.parse::<u8>().ok())
            .unwrap_or(100);
        Ok(vol)
    })
    .await
}

/// Read configured custom-models directory (HTTP `GET /custom_models_dir`).
pub async fn get_custom_models_dir() -> Result<Option<String>, String> {
    with_settings_token(|socket, token| async move {
        let resp = http_client::get_custom_models_dir(socket, &token).await?;
        if resp.status != "success" {
            return Err(resp
                .message
                .unwrap_or_else(|| "get_custom_models_dir failed".to_string()));
        }
        Ok(resp.custom_models_dir.unwrap_or(None))
    })
    .await
}

/// Set preview-typing flag (HTTP `POST /preview_typing`).
pub async fn set_preview_typing(enabled: bool) -> Result<(), String> {
    with_settings_token(move |socket, token| async move {
        let resp = http_client::set_preview_typing(socket, &token, enabled).await?;
        if resp.status != "success" {
            return Err(resp
                .message
                .unwrap_or_else(|| "set_preview_typing failed".to_string()));
        }
        Ok(())
    })
    .await
}

/// Get preview-typing flag (HTTP `GET /preview_typing`).
pub async fn get_preview_typing() -> Result<bool, String> {
    with_settings_token(|socket, token| async move {
        let resp = http_client::get_preview_typing(socket, &token).await?;
        if resp.status != "success" {
            return Err(resp
                .message
                .unwrap_or_else(|| "get_preview_typing failed".to_string()));
        }
        Ok(resp.preview_typing_enabled.unwrap_or(false))
    })
    .await
}

/// Set recording stop mode (HTTP `POST /recording_stop_mode`).
pub async fn set_recording_stop_mode(mode: String) -> Result<(), String> {
    with_settings_token(move |socket, token| {
        let mode = mode.clone();
        async move {
            let resp = http_client::set_recording_stop_mode(socket, &token, &mode).await?;
            if resp.status != "success" {
                return Err(resp
                    .message
                    .unwrap_or_else(|| "set_stop_mode failed".to_string()));
            }
            Ok(())
        }
    })
    .await
}

/// Get recording stop mode (HTTP `GET /recording_stop_mode`).
pub async fn get_recording_stop_mode() -> Result<String, String> {
    with_settings_token(|socket, token| async move {
        let resp = http_client::get_recording_stop_mode(socket, &token).await?;
        if resp.status != "success" {
            return Err(resp
                .message
                .unwrap_or_else(|| "get_stop_mode failed".to_string()));
        }
        Ok(resp
            .recording_stop_mode
            .unwrap_or_else(|| "silence-and-manual".to_string()))
    })
    .await
}

/// Set write method (HTTP `POST /write_method`).
pub async fn set_write_method(method: String) -> Result<(), String> {
    with_settings_token(move |socket, token| {
        let method = method.clone();
        async move {
            let resp = http_client::set_write_method(socket, &token, &method).await?;
            if resp.status != "success" {
                return Err(resp
                    .message
                    .unwrap_or_else(|| "set_write_method failed".to_string()));
            }
            Ok(())
        }
    })
    .await
}

/// Get write method (HTTP `GET /write_method`).
pub async fn get_write_method() -> Result<String, String> {
    with_settings_token(|socket, token| async move {
        let resp = http_client::get_write_method(socket, &token).await?;
        if resp.status != "success" {
            return Err(resp
                .message
                .unwrap_or_else(|| "get_write_method failed".to_string()));
        }
        Ok(resp.write_method.unwrap_or_else(|| "auto".to_string()))
    })
    .await
}

/// Set master volume (HTTP `POST /volume`).
pub async fn set_volume(volume: u8) -> Result<(), String> {
    with_settings_token(move |socket, token| async move {
        let resp = http_client::set_volume(socket, &token, volume).await?;
        if resp.status != "success" {
            return Err(resp
                .message
                .unwrap_or_else(|| "set_volume failed".to_string()));
        }
        Ok(())
    })
    .await
}

/// Set allow-online-models flag (HTTP `POST /allow_online_models`).
pub async fn set_allow_online_models(enabled: bool) -> Result<(), String> {
    with_settings_token(move |socket, token| async move {
        let resp = http_client::set_allow_online_models(socket, &token, enabled).await?;
        if resp.status != "success" {
            return Err(resp
                .message
                .unwrap_or_else(|| "set_allow_online failed".to_string()));
        }
        Ok(())
    })
    .await
}

/// Get allow-online-models flag (HTTP `GET /allow_online_models`).
pub async fn get_allow_online_models() -> Result<bool, String> {
    with_settings_token(|socket, token| async move {
        let resp = http_client::get_allow_online_models(socket, &token).await?;
        if resp.status != "success" {
            return Err(resp
                .message
                .unwrap_or_else(|| "get_allow_online failed".to_string()));
        }
        Ok(resp.allow_online_models.unwrap_or(false))
    })
    .await
}

/// Set custom-models directory (HTTP `POST /custom_models_dir`).
pub async fn set_custom_models_dir(path: Option<String>) -> Result<(), String> {
    with_settings_token(move |socket, token| {
        let path = path.clone();
        async move {
            let resp = http_client::set_custom_models_dir(socket, &token, path.as_deref()).await?;
            if resp.status != "success" {
                return Err(resp
                    .message
                    .unwrap_or_else(|| "set_custom_models_dir failed".to_string()));
            }
            Ok(())
        }
    })
    .await
}
