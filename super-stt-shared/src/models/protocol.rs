// SPDX-License-Identifier: GPL-3.0-only
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

use crate::models::provider::Provider;
use crate::models::recording_stop_mode::RecordingStopMode;
use crate::models::theme::AudioTheme;
use crate::models::write_method::WriteMethod;
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
    pub current_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_provider: Option<Provider>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_source: Option<crate::models::registry::SourceKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_models: Option<Vec<(String, Provider, crate::models::registry::SourceKind)>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_devices: Option<Vec<String>>,

    /// Free GPU memory in bytes (only set when CUDA is available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_free_memory: Option<u64>,

    /// Total GPU memory in bytes (only set when CUDA is available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_total_memory: Option<u64>,

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

    // Input method
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write_method: Option<String>,

    // Streaming preview text (intermediate transcription during recording)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_text: Option<String>,

    // Online models
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_online_models: Option<bool>,
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
            current_provider: None,
            current_source: None,
            available_models: None,
            available_devices: None,
            gpu_free_memory: None,
            gpu_total_memory: None,
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
            write_method: None,
            preview_text: None,
            allow_online_models: None,
        }
    }

    #[must_use]
    pub fn error(message: &str) -> Self {
        // Sanitize error messages before exposing to clients over the Unix socket.
        // Full details remain available in daemon logs.
        fn sanitize_error_message(message: &str) -> String {
            // Opt-in detailed errors for local debugging
            let debug = std::env::var("SUPER_STT_DEBUG_ERRORS").is_ok_and(|v| v == "1")
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
            current_provider: None,
            current_source: None,
            available_models: None,
            available_devices: None,
            gpu_free_memory: None,
            gpu_total_memory: None,
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
            write_method: None,
            preview_text: None,
            allow_online_models: None,
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
    pub fn with_current_model(mut self, model: impl Into<String>) -> Self {
        self.current_model = Some(model.into());
        self
    }

    #[must_use]
    pub fn with_current_provider(mut self, provider: Provider) -> Self {
        self.current_provider = Some(provider);
        self
    }

    #[must_use]
    pub fn with_current_source(mut self, source: crate::models::registry::SourceKind) -> Self {
        self.current_source = Some(source);
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
    pub fn with_available_models(
        mut self,
        models: Vec<(String, Provider, crate::models::registry::SourceKind)>,
    ) -> Self {
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
    pub fn with_gpu_free_memory(mut self, bytes: u64) -> Self {
        self.gpu_free_memory = Some(bytes);
        self
    }

    #[must_use]
    pub fn with_gpu_total_memory(mut self, bytes: u64) -> Self {
        self.gpu_total_memory = Some(bytes);
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

    #[must_use]
    pub fn with_write_method(mut self, method: String) -> Self {
        self.write_method = Some(method);
        self
    }

    #[must_use]
    pub fn with_preview_text(mut self, text: String) -> Self {
        self.preview_text = Some(text);
        self
    }

    #[must_use]
    pub fn with_allow_online_models(mut self, allowed: bool) -> Self {
        self.allow_online_models = Some(allowed);
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
        wait: bool,
        preview: Option<bool>,
    },
    SetAudioTheme {
        theme: String,
    },
    GetAudioTheme,
    TestAudioTheme,
    SetModel {
        model: String,
        provider: Provider,
        source: crate::models::registry::SourceKind,
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
    SetWriteMethod {
        method: WriteMethod,
    },
    GetWriteMethod,
    SetVolume {
        volume: u8,
    },
    GetVolume,
    SetAllowOnlineModels {
        enabled: bool,
    },
    GetAllowOnlineModels,
    SetCustomModelsDir {
        path: Option<String>,
    },
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
                ..
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
                ..
            } => {
                assert!(write_mode);
                assert_eq!(stop_mode, None);
            }
            _ => panic!("expected Command::Record"),
        }
    }

    #[test]
    fn record_command_wait_true() {
        let request = make_request(
            "record",
            Some(json!({
                "write_mode": false,
                "stop_mode": "manual-only",
                "wait": true,
            })),
        );
        let command = Command::try_from(request).expect("record command should parse");
        match command {
            Command::Record { wait, .. } => assert!(wait),
            _ => panic!("expected Command::Record"),
        }
    }

    #[test]
    fn record_command_wait_defaults_to_false() {
        let request = make_request("record", Some(json!({ "write_mode": false })));
        let command = Command::try_from(request).expect("record command should parse");
        match command {
            Command::Record { wait, .. } => assert!(!wait),
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
    fn set_allow_online_models_parses() {
        let mut request = make_request("set_allow_online_models", None);
        request.enabled = Some(true);
        let command = Command::try_from(request).expect("command should parse");
        match command {
            Command::SetAllowOnlineModels { enabled } => assert!(enabled),
            _ => panic!("expected Command::SetAllowOnlineModels"),
        }
    }

    #[test]
    fn set_allow_online_models_missing_enabled_fails() {
        let request = make_request("set_allow_online_models", None);
        let result = Command::try_from(request);
        assert!(result.is_err());
    }

    #[test]
    fn get_allow_online_models_parses() {
        let request = make_request("get_allow_online_models", None);
        let command = Command::try_from(request).expect("command should parse");
        assert!(matches!(command, Command::GetAllowOnlineModels));
    }

    #[test]
    fn response_with_allow_online_models() {
        let response = DaemonResponse::success().with_allow_online_models(true);
        assert_eq!(response.allow_online_models, Some(true));

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["allow_online_models"], true);
    }

    #[test]
    fn set_allow_online_models_false() {
        let mut request = make_request("set_allow_online_models", None);
        request.enabled = Some(false);
        let command = Command::try_from(request).expect("command should parse");
        match command {
            Command::SetAllowOnlineModels { enabled } => assert!(!enabled),
            _ => panic!("expected Command::SetAllowOnlineModels"),
        }
    }

    #[test]
    fn response_allow_online_models_false_serializes() {
        let response = DaemonResponse::success().with_allow_online_models(false);
        assert_eq!(response.allow_online_models, Some(false));

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["allow_online_models"], false);
    }

    #[test]
    fn response_allow_online_models_skipped_when_none() {
        let response = DaemonResponse::success();
        assert_eq!(response.allow_online_models, None);

        let json = serde_json::to_value(&response).unwrap();
        assert!(json.get("allow_online_models").is_none());
    }

    #[test]
    fn set_model_parses_online_models() {
        let cases: &[(&str, &str)] = &[
            ("whisper-1", "openai"),
            ("gpt-4o-transcribe", "openai"),
            ("gpt-4o-mini-transcribe", "openai"),
            ("voxtral-mini-latest", "mistral"),
            ("nova-3", "deepgram"),
        ];
        for (model_name, provider_str) in cases {
            let request = make_request(
                "set_model",
                Some(json!({ "model": model_name, "provider": provider_str })),
            );
            let command = Command::try_from(request)
                .unwrap_or_else(|e| panic!("set_model should parse {model_name}: {e}"));
            match command {
                Command::SetModel {
                    model,
                    provider,
                    source,
                } => {
                    assert_eq!(model.to_string(), *model_name);
                    assert!(matches!(provider, Provider::Online(_)), "{model_name}");
                    assert_eq!(source, crate::models::registry::SourceKind::Online);
                }
                _ => panic!("expected Command::SetModel for {model_name}"),
            }
        }
    }

    #[test]
    fn set_model_parses_local_name() {
        let request = make_request(
            "set_model",
            Some(json!({ "model": "whisper-tiny", "provider": "local-whisper" })),
        );
        let command = Command::try_from(request).expect("should parse");
        match command {
            Command::SetModel {
                model,
                provider,
                source,
            } => {
                assert_eq!(model, "whisper-tiny");
                assert_eq!(provider, crate::models::provider::Provider::LocalWhisper);
                assert_eq!(source, crate::models::registry::SourceKind::Builtin);
            }
            _ => panic!("expected Command::SetModel"),
        }
    }

    #[test]
    fn set_model_with_explicit_custom_source() {
        let request = make_request(
            "set_model",
            Some(json!({
                "model": "whisper-tiny",
                "provider": "local-whisper",
                "source": "custom",
            })),
        );
        let command = Command::try_from(request).expect("should parse");
        match command {
            Command::SetModel { source, .. } => {
                assert_eq!(source, crate::models::registry::SourceKind::Custom);
            }
            _ => panic!("expected Command::SetModel"),
        }
    }

    #[test]
    fn set_model_rejects_missing_provider() {
        let request = make_request("set_model", Some(json!({ "model": "whisper-tiny" })));
        let result = Command::try_from(request);
        assert!(
            result.is_err(),
            "set_model without provider should be rejected"
        );
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

    #[test]
    fn set_custom_models_dir_parses_with_path() {
        let request = make_request(
            "set_custom_models_dir",
            Some(json!({ "path": "/tmp/models" })),
        );
        let command = Command::try_from(request).expect("command should parse");
        match command {
            Command::SetCustomModelsDir { path } => {
                assert_eq!(path.as_deref(), Some("/tmp/models"));
            }
            _ => panic!("expected Command::SetCustomModelsDir"),
        }
    }

    #[test]
    fn set_custom_models_dir_parses_with_null() {
        let request = make_request("set_custom_models_dir", Some(json!({ "path": null })));
        let command = Command::try_from(request).expect("command should parse");
        match command {
            Command::SetCustomModelsDir { path } => {
                assert!(path.is_none());
            }
            _ => panic!("expected Command::SetCustomModelsDir"),
        }
    }

    #[test]
    fn set_custom_models_dir_parses_without_data() {
        let request = make_request("set_custom_models_dir", None);
        let command = Command::try_from(request).expect("command should parse");
        match command {
            Command::SetCustomModelsDir { path } => {
                assert!(path.is_none());
            }
            _ => panic!("expected Command::SetCustomModelsDir"),
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
            "set_write_method" => cmd_set_write_method(&request),
            "get_write_method" => Ok(Command::GetWriteMethod),
            "set_volume" => cmd_set_volume(&request),
            "get_volume" => Ok(Command::GetVolume),
            "set_allow_online_models" => cmd_set_allow_online_models(&request),
            "get_allow_online_models" => Ok(Command::GetAllowOnlineModels),
            "set_custom_models_dir" => Ok(cmd_set_custom_models_dir(&request)),
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
    let wait = request
        .data
        .as_ref()
        .and_then(|data| data.get("wait"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let preview = request
        .data
        .as_ref()
        .and_then(|data| data.get("preview"))
        .and_then(serde_json::Value::as_bool);
    Command::Record {
        write_mode,
        stop_mode,
        wait,
        preview,
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
    let data = request.data.as_ref();
    let model_str = data
        .and_then(|d| d.get("model"))
        .and_then(|v| v.as_str())
        .ok_or("Model string is empty")?;

    let provider_str = data
        .and_then(|d| d.get("provider"))
        .and_then(|v| v.as_str())
        .ok_or("Provider string is required for set_model")?;
    let provider: Provider = provider_str
        .parse()
        .map_err(|e| format!("Invalid provider {provider_str:?}: {e}"))?;

    let source = match data.and_then(|d| d.get("source")).and_then(|v| v.as_str()) {
        Some(s) => s
            .parse()
            .map_err(|e| format!("Invalid source {s:?}: {e}"))?,
        None => {
            // For backward compat: derive a sensible default from provider.
            // Online providers always have source=Online; local providers
            // default to Builtin (the registry path).
            if matches!(provider, Provider::Online(_)) {
                crate::models::registry::SourceKind::Online
            } else {
                crate::models::registry::SourceKind::Builtin
            }
        }
    };

    Ok(Command::SetModel {
        model: model_str.to_string(),
        provider,
        source,
    })
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

fn cmd_set_write_method(request: &DaemonRequest) -> Result<Command, String> {
    let method_str = request
        .data
        .as_ref()
        .and_then(|data| data.get("method"))
        .and_then(|v| v.as_str())
        .ok_or("Missing method for set_write_method command")?;
    let method = method_str
        .parse::<WriteMethod>()
        .map_err(|e| format!("Invalid input method: {e}"))?;
    Ok(Command::SetWriteMethod { method })
}

fn cmd_set_allow_online_models(request: &DaemonRequest) -> Result<Command, String> {
    let enabled = request
        .enabled
        .ok_or("Missing enabled field for set_allow_online_models command")?;
    Ok(Command::SetAllowOnlineModels { enabled })
}

fn cmd_set_volume(request: &DaemonRequest) -> Result<Command, String> {
    let volume = request
        .data
        .as_ref()
        .and_then(|data| data.get("volume"))
        .and_then(serde_json::Value::as_u64)
        .ok_or("Missing volume for set_volume command")?;
    let volume =
        u8::try_from(volume).map_err(|_| "Volume must be between 0 and 100".to_string())?;
    if volume > 100 {
        return Err("Volume must be between 0 and 100".to_string());
    }
    Ok(Command::SetVolume { volume })
}

fn cmd_set_custom_models_dir(request: &DaemonRequest) -> Command {
    let path = request
        .data
        .as_ref()
        .and_then(|data| data.get("path"))
        .and_then(|v| v.as_str())
        .map(String::from);
    Command::SetCustomModelsDir { path }
}
