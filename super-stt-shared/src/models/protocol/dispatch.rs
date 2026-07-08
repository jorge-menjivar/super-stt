// SPDX-License-Identifier: GPL-3.0-only
use super::command::Command;
use super::request::DaemonRequest;
use crate::models::provider::Provider;
use crate::models::recording_stop_mode::RecordingStopMode;
use crate::models::write_method::WriteMethod;
use crate::validation::{self, Validate};

impl TryFrom<DaemonRequest> for Command {
    type Error = String;

    fn try_from(request: DaemonRequest) -> Result<Self, Self::Error> {
        // Validate the request first
        if let Err(e) = request.validate() {
            return Err(format!("Request validation failed: {e}"));
        }
        match request.command.as_str() {
            "transcribe" => cmd_transcribe(&request),
            "ping" => Ok(Command::Ping {
                client_id: request.client_id.clone(),
            }),
            "status" => Ok(Command::Status),
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
            "set_primary_language" => cmd_set_primary_language(&request),
            "get_primary_language" => Ok(Command::GetPrimaryLanguage),
            "clear_primary_language" => Ok(Command::ClearPrimaryLanguage),
            "set_model_language" => cmd_set_model_language(&request),
            "get_model_language" => cmd_get_model_language(&request),
            "clear_model_language" => cmd_clear_model_language(&request),
            "set_allow_online_models" => cmd_set_allow_online_models(&request),
            "get_allow_online_models" => Ok(Command::GetAllowOnlineModels),
            "set_custom_models_dir" => Ok(cmd_set_custom_models_dir(&request)),
            "get_custom_models_dir" => Ok(Command::GetCustomModelsDir),
            "list_backends" => Ok(Command::ListBackends),
            "reload_active_model" => Ok(Command::ReloadActiveModel),
            "unload_active_model" => Ok(Command::UnloadActiveModel),
            "set_backend_option" => cmd_set_backend_option(&request),
            "set_active_backend" => cmd_set_active_backend(&request),
            "get_active_backend" => Ok(Command::GetActiveBackend),
            "clear_active_backend" => Ok(Command::ClearActiveBackend),
            "get_gpu_info" => Ok(Command::GetGpuInfo),
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

    // `source` is the serving backend's repo id. It is optional: when absent
    // the daemon selects the first installed backend serving `(model,
    // provider)`.
    let source = data
        .and_then(|d| d.get("source"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

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

fn cmd_set_backend_option(request: &DaemonRequest) -> Result<Command, String> {
    let data = request.data.as_ref();
    let source = data
        .and_then(|d| d.get("source"))
        .and_then(|v| v.as_str())
        .ok_or("Missing source for set_backend_option")?
        .to_string();
    let name = data
        .and_then(|d| d.get("name"))
        .and_then(|v| v.as_str())
        .ok_or("Missing name for set_backend_option")?
        .to_string();
    // Empty/absent value clears the override.
    let value = data
        .and_then(|d| d.get("value"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    Ok(Command::SetBackendOption {
        source,
        name,
        value,
    })
}

fn cmd_set_active_backend(request: &DaemonRequest) -> Result<Command, String> {
    let source = request
        .data
        .as_ref()
        .and_then(|d| d.get("source"))
        .and_then(|v| v.as_str())
        .ok_or("Missing source for set_active_backend")?
        .to_string();
    Ok(Command::SetActiveBackend { source })
}

fn cmd_set_primary_language(request: &DaemonRequest) -> Result<Command, String> {
    let language = request
        .data
        .as_ref()
        .and_then(|data| data.get("language"))
        .and_then(|v| v.as_str())
        .ok_or("Missing language for set_primary_language command")?
        .to_string();
    Ok(Command::SetPrimaryLanguage { language })
}

/// Extract the `(source, model)` pair every per-model language command carries
/// in `data`. Both are required.
fn model_language_target(
    request: &DaemonRequest,
    command: &str,
) -> Result<(String, String), String> {
    let data = request.data.as_ref();
    let source = data
        .and_then(|d| d.get("source"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("Missing source for {command} command"))?
        .to_string();
    let model = data
        .and_then(|d| d.get("model"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("Missing model for {command} command"))?
        .to_string();
    Ok((source, model))
}

fn cmd_set_model_language(request: &DaemonRequest) -> Result<Command, String> {
    let (source, model) = model_language_target(request, "set_model_language")?;
    let language = request
        .data
        .as_ref()
        .and_then(|data| data.get("language"))
        .and_then(|v| v.as_str())
        .ok_or("Missing language for set_model_language command")?
        .to_string();
    Ok(Command::SetModelLanguage {
        source,
        model,
        language,
    })
}

fn cmd_get_model_language(request: &DaemonRequest) -> Result<Command, String> {
    let (source, model) = model_language_target(request, "get_model_language")?;
    Ok(Command::GetModelLanguage { source, model })
}

fn cmd_clear_model_language(request: &DaemonRequest) -> Result<Command, String> {
    let (source, model) = model_language_target(request, "clear_model_language")?;
    Ok(Command::ClearModelLanguage { source, model })
}
