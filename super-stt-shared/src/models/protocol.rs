// SPDX-License-Identifier: GPL-3.0-only
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, str::FromStr};

use crate::models::recording_stop_mode::RecordingStopMode;
use crate::models::theme::AudioTheme;
use crate::stt_model::STTModel;
use crate::validation::{self, Validate, ValidationError};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DaemonRequest {
    pub command: String,
    #[serde(default)]
    pub audio_data: Option<Vec<f32>>,
    #[serde(default)]
    pub sample_rate: Option<u32>,
    #[serde(default)]
    pub client_id: Option<String>,

    // Notification system fields
    #[serde(default)]
    pub event_types: Option<Vec<String>>,
    #[serde(default)]
    pub client_info: Option<HashMap<String, Value>>,
    #[serde(default)]
    pub since_timestamp: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub event_type: Option<String>,
    #[serde(default)]
    pub data: Option<Value>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DaemonResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcription: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_loaded: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_model: Option<STTModel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_models: Option<Vec<STTModel>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_devices: Option<Vec<String>>,

    // Notification system fields
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscribed_to: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_subscribers: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<NotificationEvent>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscriber_info: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notification_info: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,

    // Audio theme fields
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_theme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_audio_themes: Option<Vec<AudioTheme>>,

    // Download progress fields
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_progress: Option<DownloadProgress>,

    // Daemon configuration fields
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daemon_config: Option<Value>,

    // Connection status fields
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_active: Option<bool>,

    // Preview typing fields
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_typing_enabled: Option<bool>,

    // Recording stop mode
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recording_stop_mode: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DownloadProgress {
    pub model_name: String,
    pub current_file: String,
    pub file_index: usize,
    pub total_files: usize,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub percentage: f32,
    pub status: String, // "downloading", "cancelled", "completed", "error"
    pub started_at: String,
    pub eta_seconds: Option<u64>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NotificationEvent {
    #[serde(rename = "type")]
    pub event_type_field: String,
    pub event_type: String,
    pub client_id: String,
    pub timestamp: String,
    pub data: Value,
}

impl DaemonResponse {
    /// Canonical message returned when a running recording is stopped via a second press.
    pub const RECORDING_STOP_SIGNAL_MSG: &str = "Recording stop signal sent";

    #[must_use]
    pub fn success() -> Self {
        Self {
            status: "success".to_string(),
            message: None,
            transcription: None,
            device: None,
            model_loaded: None,
            current_model: None,
            available_models: None,
            available_devices: None,
            subscribed_to: None,
            total_subscribers: None,
            events: None,
            count: None,
            subscriber_info: None,
            notification_info: None,
            client_id: None,
            audio_theme: None,
            available_audio_themes: None,
            download_progress: None,
            daemon_config: None,
            connection_active: None,
            preview_typing_enabled: None,
            recording_stop_mode: None,
        }
    }

    #[must_use]
    pub fn error(message: &str) -> Self {
        // Sanitize error messages before exposing to clients over the Unix socket.
        // Full details remain available in daemon logs.
        fn sanitize_error_message(message: &str) -> String {
            // Opt-in detailed errors for local debugging
            let debug = std::env::var("SUPER_STT_DEBUG_ERRORS")
                .map(|v| matches!(v.as_str(), "1"))
                .unwrap_or(false)
                || cfg!(debug_assertions);
            if debug {
                return message.to_string();
            }

            // Keep only the first line and trim internal details after a colon
            let first_line = message.lines().next().unwrap_or(message).trim();
            if let Some((prefix, _)) = first_line.split_once(':') {
                prefix.trim().to_string()
            } else {
                first_line.to_string()
            }
        }

        Self {
            status: "error".to_string(),
            message: Some(sanitize_error_message(message)),
            transcription: None,
            device: None,
            model_loaded: None,
            current_model: None,
            available_models: None,
            available_devices: None,
            subscribed_to: None,
            total_subscribers: None,
            events: None,
            count: None,
            subscriber_info: None,
            notification_info: None,
            client_id: None,
            audio_theme: None,
            available_audio_themes: None,
            download_progress: None,
            daemon_config: None,
            connection_active: None,
            preview_typing_enabled: None,
            recording_stop_mode: None,
        }
    }

    #[must_use]
    pub fn with_transcription(mut self, transcription: String) -> Self {
        self.transcription = Some(transcription);
        self
    }

    #[must_use]
    pub fn with_device(mut self, device: String) -> Self {
        self.device = Some(device);
        self
    }

    #[must_use]
    pub fn with_model_loaded(mut self, loaded: bool) -> Self {
        self.model_loaded = Some(loaded);
        self
    }

    #[must_use]
    pub fn with_current_model(mut self, model: STTModel) -> Self {
        self.current_model = Some(model);
        self
    }

    #[must_use]
    pub fn with_message(mut self, message: String) -> Self {
        self.message = Some(message);
        self
    }

    #[must_use]
    pub fn with_client_id(mut self, client_id: String) -> Self {
        self.client_id = Some(client_id);
        self
    }

    #[must_use]
    pub fn with_subscribed_to(mut self, events: Vec<String>) -> Self {
        self.subscribed_to = Some(events);
        self
    }

    #[must_use]
    pub fn with_total_subscribers(mut self, count: u32) -> Self {
        self.total_subscribers = Some(count);
        self
    }

    #[must_use]
    pub fn with_events(mut self, events: Vec<NotificationEvent>) -> Self {
        self.events = Some(events);
        self
    }

    #[must_use]
    pub fn with_notification_info(mut self, info: Value) -> Self {
        self.notification_info = Some(info);
        self
    }

    #[must_use]
    pub fn with_audio_theme(mut self, theme: String) -> Self {
        self.audio_theme = Some(theme);
        self
    }

    #[must_use]
    pub fn with_available_audio_themes(mut self, themes: Vec<AudioTheme>) -> Self {
        self.available_audio_themes = Some(themes);
        self
    }

    #[must_use]
    pub fn with_available_models(mut self, models: Vec<STTModel>) -> Self {
        self.available_models = Some(models);
        self
    }

    #[must_use]
    pub fn with_download_progress(mut self, progress: DownloadProgress) -> Self {
        self.download_progress = Some(progress);
        self
    }

    #[must_use]
    pub fn with_available_devices(mut self, devices: Vec<String>) -> Self {
        self.available_devices = Some(devices);
        self
    }

    #[must_use]
    pub fn with_daemon_config(mut self, config: Value) -> Self {
        self.daemon_config = Some(config);
        self
    }

    #[must_use]
    pub fn with_connection_active(mut self, active: bool) -> Self {
        self.connection_active = Some(active);
        self
    }

    #[must_use]
    pub fn with_preview_typing_enabled(mut self, enabled: bool) -> Self {
        self.preview_typing_enabled = Some(enabled);
        self
    }

    #[must_use]
    pub fn with_recording_stop_mode(mut self, mode: String) -> Self {
        self.recording_stop_mode = Some(mode);
        self
    }
}

#[derive(Debug)]
pub enum Command {
    Transcribe {
        audio_data: Vec<f32>,
        sample_rate: u32,
        client_id: String,
    },
    Subscribe {
        event_types: Vec<String>,
        client_info: HashMap<String, Value>,
    },
    Unsubscribe,
    GetEvents {
        since_timestamp: Option<String>,
        event_types: Option<Vec<String>>,
        limit: u32,
    },
    GetSubscriberInfo,
    Notify {
        event_type: String,
        client_id: String,
        data: Value,
    },
    Ping {
        client_id: Option<String>,
    },
    Status,
    StartRealTimeTranscription {
        client_id: String,
        sample_rate: Option<u32>,
        language: Option<String>,
    },
    RealTimeAudioChunk {
        client_id: String,
        audio_data: Vec<f32>,
        sample_rate: u32,
    },
    Record {
        write_mode: bool,
        stop_mode: Option<RecordingStopMode>,
    },
    SetAudioTheme {
        theme: String,
    },
    GetAudioTheme,
    TestAudioTheme,
    SetModel {
        model: STTModel,
    },
    GetModel,
    ListModels,
    SetDevice {
        device: String, // "cpu" or "cuda"
    },
    GetDevice,
    GetConfig,
    CancelDownload,
    GetDownloadStatus,
    ListAudioThemes,
    SetPreviewTyping {
        enabled: bool,
    },
    GetPreviewTyping,
    SetRecordingStopMode {
        mode: RecordingStopMode,
    },
    GetRecordingStopMode,
}

impl Validate for DaemonRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        // Validate command string
        validation::validate_command(&self.command)?;

        // Validate audio data if present
        if let Some(ref audio_data) = self.audio_data {
            validation::validate_audio_data(audio_data)?;
        }

        // Validate sample rate if present
        if let Some(sample_rate) = self.sample_rate {
            validation::validate_sample_rate(sample_rate)?;
        }

        // Validate string fields
        validation::validate_optional_string(
            &self.client_id,
            "client_id",
            validation::limits::MAX_STRING_LENGTH,
        )?;
        validation::validate_optional_string(
            &self.since_timestamp,
            "since_timestamp",
            validation::limits::MAX_STRING_LENGTH,
        )?;
        validation::validate_optional_string(
            &self.event_type,
            "event_type",
            validation::limits::MAX_NAME_LENGTH,
        )?;
        validation::validate_optional_string(
            &self.language,
            "language",
            validation::limits::MAX_NAME_LENGTH,
        )?;

        // Validate event types if present
        if let Some(ref event_types) = self.event_types {
            validation::validate_event_types(event_types)?;
        }

        // Validate limit if present
        if let Some(limit) = self.limit {
            validation::validate_limit(limit)?;
        }

        // Validate JSON data if present
        if let Some(ref data) = self.data {
            validation::validate_json_value(data)?;
        }

        // Validate client_info if present
        if let Some(ref client_info) = self.client_info {
            for value in client_info.values() {
                validation::validate_json_value(value)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_request(command: &str, data: Option<Value>) -> DaemonRequest {
        DaemonRequest {
            command: command.to_string(),
            audio_data: None,
            sample_rate: None,
            client_id: None,
            event_types: None,
            client_info: None,
            since_timestamp: None,
            limit: None,
            event_type: None,
            data,
            language: None,
            enabled: None,
        }
    }

    #[test]
    fn record_command_parses_stop_mode() {
        let request = make_request(
            "record",
            Some(json!({
                "write_mode": false,
                "stop_mode": "manual-only",
            })),
        );
        let command = Command::try_from(request).expect("record command should parse");
        match command {
            Command::Record {
                write_mode,
                stop_mode,
            } => {
                assert!(!write_mode);
                assert_eq!(stop_mode, Some(RecordingStopMode::ManualOnly));
            }
            _ => panic!("expected Command::Record"),
        }
    }

    #[test]
    fn record_command_without_stop_mode_defaults_to_none() {
        let request = make_request("record", Some(json!({ "write_mode": true })));
        let command = Command::try_from(request).expect("record command should parse");
        match command {
            Command::Record {
                write_mode,
                stop_mode,
            } => {
                assert!(write_mode);
                assert_eq!(stop_mode, None);
            }
            _ => panic!("expected Command::Record"),
        }
    }

    #[test]
    fn record_command_backward_compat_disable_silence_detection() {
        let request = make_request(
            "record",
            Some(json!({
                "write_mode": false,
                "disable_silence_detection": true,
            })),
        );
        let command = Command::try_from(request).expect("record command should parse");
        match command {
            Command::Record { stop_mode, .. } => {
                assert_eq!(stop_mode, Some(RecordingStopMode::ManualOnly));
            }
            _ => panic!("expected Command::Record"),
        }
    }

    #[test]
    fn set_recording_stop_mode_parses() {
        let request = make_request(
            "set_recording_stop_mode",
            Some(json!({ "mode": "silence-only" })),
        );
        let command = Command::try_from(request).expect("command should parse");
        match command {
            Command::SetRecordingStopMode { mode } => {
                assert_eq!(mode, RecordingStopMode::SilenceOnly);
            }
            _ => panic!("expected Command::SetRecordingStopMode"),
        }
    }
}

impl TryFrom<DaemonRequest> for Command {
    type Error = String;

    fn try_from(request: DaemonRequest) -> Result<Self, Self::Error> {
        // Validate the request first
        if let Err(e) = request.validate() {
            return Err(format!("Request validation failed: {e}"));
        }
        match request.command.as_str() {
            "transcribe" => cmd_transcribe(&request),
            "subscribe" => cmd_subscribe(&request),
            "unsubscribe" => Ok(Command::Unsubscribe),
            "get_events" => cmd_get_events(&request),
            "get_subscriber_info" => Ok(Command::GetSubscriberInfo),
            "notify" => cmd_notify(&request),
            "ping" => Ok(Command::Ping {
                client_id: request.client_id.clone(),
            }),
            "status" => Ok(Command::Status),
            "start_realtime" => Ok(cmd_start_realtime(&request)),
            "realtime_audio" => cmd_realtime_audio(&request),
            "record" => Ok(cmd_record(&request)),
            "set_audio_theme" => cmd_set_audio_theme(&request),
            "get_audio_theme" => Ok(Command::GetAudioTheme),
            "test_audio_theme" => Ok(Command::TestAudioTheme),
            "set_model" => cmd_set_model(&request),
            "get_model" => Ok(Command::GetModel),
            "list_models" => Ok(Command::ListModels),
            "set_device" => cmd_set_device(&request),
            "get_device" => Ok(Command::GetDevice),
            "get_config" => Ok(Command::GetConfig),
            "cancel_download" => Ok(Command::CancelDownload),
            "get_download_status" => Ok(Command::GetDownloadStatus),
            "list_audio_themes" => Ok(Command::ListAudioThemes),
            "set_preview_typing" => cmd_set_preview_typing(&request),
            "get_preview_typing" => Ok(Command::GetPreviewTyping),
            "set_recording_stop_mode" => cmd_set_recording_stop_mode(&request),
            "get_recording_stop_mode" => Ok(Command::GetRecordingStopMode),
            _ => Err(format!("Unknown command: {}", request.command)),
        }
    }
}

fn cmd_transcribe(request: &DaemonRequest) -> Result<Command, String> {
    let audio_data = request
        .audio_data
        .clone()
        .ok_or("Missing audio_data for transcribe command")?;
    let sample_rate = request.sample_rate.unwrap_or(16000);
    let client_id = request
        .client_id
        .clone()
        .unwrap_or_else(|| format!("client_{}", uuid::Uuid::new_v4()));
    Ok(Command::Transcribe {
        audio_data,
        sample_rate,
        client_id,
    })
}

fn cmd_subscribe(request: &DaemonRequest) -> Result<Command, String> {
    let event_types = request
        .event_types
        .clone()
        .ok_or("Missing event_types for subscribe command")?;
    let client_info = request.client_info.clone().unwrap_or_default();
    Ok(Command::Subscribe {
        event_types,
        client_info,
    })
}

fn cmd_get_events(request: &DaemonRequest) -> Result<Command, String> {
    let limit = request.limit.unwrap_or(100);
    if let Err(e) = validation::validate_limit(limit) {
        return Err(e.to_string());
    }
    Ok(Command::GetEvents {
        since_timestamp: request.since_timestamp.clone(),
        event_types: request.event_types.clone(),
        limit,
    })
}

fn cmd_notify(request: &DaemonRequest) -> Result<Command, String> {
    let event_type = request
        .event_type
        .clone()
        .ok_or("Missing event_type for notify command")?;
    let client_id = request
        .client_id
        .clone()
        .ok_or("Missing client_id for notify command")?;
    let data = request
        .data
        .clone()
        .ok_or("Missing data for notify command")?;
    Ok(Command::Notify {
        event_type,
        client_id,
        data,
    })
}

fn cmd_start_realtime(request: &DaemonRequest) -> Command {
    let client_id = request
        .client_id
        .clone()
        .unwrap_or_else(|| format!("realtime_{}", uuid::Uuid::new_v4()));
    Command::StartRealTimeTranscription {
        client_id,
        sample_rate: request.sample_rate,
        language: request.language.clone(),
    }
}

fn cmd_realtime_audio(request: &DaemonRequest) -> Result<Command, String> {
    let client_id = request
        .client_id
        .clone()
        .ok_or("Missing client_id for realtime_audio command")?;
    let audio_data = request
        .audio_data
        .clone()
        .ok_or("Missing audio_data for realtime_audio command")?;
    let sample_rate = request.sample_rate.unwrap_or(16000);
    Ok(Command::RealTimeAudioChunk {
        client_id,
        audio_data,
        sample_rate,
    })
}

fn cmd_record(request: &DaemonRequest) -> Command {
    let write_mode = request
        .data
        .as_ref()
        .and_then(|data| data.get("write_mode"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    // Parse stop_mode string if present
    let stop_mode = request
        .data
        .as_ref()
        .and_then(|data| data.get("stop_mode"))
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<RecordingStopMode>().ok())
        // Backward compat: if stop_mode absent, check legacy disable_silence_detection
        .or_else(|| {
            let disabled = request
                .data
                .as_ref()
                .and_then(|data| data.get("disable_silence_detection"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if disabled {
                Some(RecordingStopMode::ManualOnly)
            } else {
                None
            }
        });
    Command::Record {
        write_mode,
        stop_mode,
    }
}

fn cmd_set_audio_theme(request: &DaemonRequest) -> Result<Command, String> {
    let theme = request
        .data
        .as_ref()
        .and_then(|data| data.get("theme"))
        .and_then(|v| v.as_str())
        .ok_or("Missing theme for set_audio_theme command")?
        .to_string();

    if let Err(e) =
        validation::validate_string(&theme, "theme", validation::limits::MAX_NAME_LENGTH)
    {
        return Err(e.to_string());
    }

    Ok(Command::SetAudioTheme { theme })
}

fn cmd_set_model(request: &DaemonRequest) -> Result<Command, String> {
    let model_value = request.data.as_ref().and_then(|data| data.get("model"));
    let model_str = model_value.and_then(|v| v.as_str());
    if let Some(model_str) = model_str {
        match STTModel::from_str(model_str) {
            Ok(model) => Ok(Command::SetModel { model }),
            Err(err) => Err(format!("Failed to parse model: {err}")),
        }
    } else {
        Err("Model string is empty".to_string())
    }
}

fn cmd_set_device(request: &DaemonRequest) -> Result<Command, String> {
    let device = request
        .data
        .as_ref()
        .and_then(|data| data.get("device"))
        .and_then(|v| v.as_str())
        .ok_or("Missing device for set_device command")?
        .to_string();

    if let Err(e) =
        validation::validate_string(&device, "device", validation::limits::MAX_NAME_LENGTH)
    {
        return Err(e.to_string());
    }

    Ok(Command::SetDevice { device })
}

fn cmd_set_preview_typing(request: &DaemonRequest) -> Result<Command, String> {
    let enabled = request
        .enabled
        .ok_or("Missing enabled field for set_preview_typing command")?;

    Ok(Command::SetPreviewTyping { enabled })
}

fn cmd_set_recording_stop_mode(request: &DaemonRequest) -> Result<Command, String> {
    let mode_str = request
        .data
        .as_ref()
        .and_then(|data| data.get("mode"))
        .and_then(|v| v.as_str())
        .ok_or("Missing mode for set_recording_stop_mode command")?;
    let mode = mode_str
        .parse::<RecordingStopMode>()
        .map_err(|e| format!("Invalid recording stop mode: {e}"))?;
    Ok(Command::SetRecordingStopMode { mode })
}
