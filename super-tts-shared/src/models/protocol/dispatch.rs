// SPDX-License-Identifier: GPL-3.0-only
use super::command::Command;
use super::request::DaemonRequest;
use crate::validation::{self, Validate};

impl TryFrom<DaemonRequest> for Command {
    type Error = String;

    fn try_from(request: DaemonRequest) -> Result<Self, Self::Error> {
        // Validate the request first
        if let Err(e) = request.validate() {
            return Err(format!("Request validation failed: {e}"));
        }
        match request.command.as_str() {
            "speak" => cmd_speak(&request),
            "stop_speaking" => Ok(Command::StopSpeaking),
            "ping" => Ok(Command::Ping {
                client_id: request.client_id.clone(),
            }),
            "status" => Ok(Command::Status),
            "set_audio_theme" => cmd_set_audio_theme(&request),
            "get_audio_theme" => Ok(Command::GetAudioTheme),
            "test_audio_theme" => Ok(Command::TestAudioTheme),
            "set_model" => cmd_set_model(&request),
            "get_model" => Ok(Command::GetModel),
            "list_models" => Ok(Command::ListModels),
            "set_device" => cmd_set_device(&request),
            "get_device" => Ok(Command::GetDevice),
            "set_model_device" => cmd_set_model_device(&request),
            "get_model_device" => cmd_get_model_device(&request),
            "list_model_devices" => Ok(Command::ListModelDevices {
                model: model_device_target(&request, "list_model_devices")?,
            }),
            "list_active_backend_devices" => Ok(Command::ListActiveBackendDevices),
            "get_config" => Ok(Command::GetConfig),
            "cancel_download" => Ok(Command::CancelDownload),
            "get_download_status" => Ok(Command::GetDownloadStatus),
            "list_audio_themes" => Ok(Command::ListAudioThemes),
            "set_notification_method" => cmd_set_notification_method(&request),
            "get_notification_method" => Ok(Command::GetNotificationMethod),
            "set_update_check_enabled" => cmd_set_update_check_enabled(&request),
            "get_update_check_enabled" => Ok(Command::GetUpdateCheckEnabled),
            "set_update_beta_optin" => cmd_set_update_beta_optin(&request),
            "get_update_beta_optin" => Ok(Command::GetUpdateBetaOptin),
            "set_volume" => cmd_set_volume(&request),
            "get_volume" => Ok(Command::GetVolume),
            "set_primary_language" => cmd_set_primary_language(&request),
            "get_primary_language" => Ok(Command::GetPrimaryLanguage),
            "clear_primary_language" => Ok(Command::ClearPrimaryLanguage),
            "list_primary_languages" => Ok(Command::ListPrimaryLanguages),
            "set_model_language" => cmd_set_model_language(&request),
            "get_model_language" => cmd_get_model_language(&request),
            "clear_model_language" => cmd_clear_model_language(&request),
            "list_model_languages" => cmd_list_model_languages(&request),
            "set_model_voice" => cmd_set_model_voice(&request),
            "get_model_voice" => cmd_get_model_voice(&request),
            "clear_model_voice" => cmd_clear_model_voice(&request),
            "list_model_voices" => cmd_list_model_voices(&request),
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
            "get_pipeline" => Ok(Command::GetPipeline),
            "get_gpu_info" => Ok(Command::GetGpuInfo),
            _ => Err(format!("Unknown command: {}", request.command)),
        }
    }
}

/// Build a `speak` command. `text` is required; everything else refines how it
/// is spoken and is optional, so a bare `{"text": "..."}` is a valid request.
fn cmd_speak(request: &DaemonRequest) -> Result<Command, String> {
    let text = request
        .data
        .as_ref()
        .and_then(|d| d.get("text"))
        .and_then(serde_json::Value::as_str)
        .ok_or("Missing text for speak command")?
        .to_string();
    Ok(Command::Speak { text })
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

    // `source` is the serving backend's repo id. It is optional: when absent
    // the daemon resolves it against the active backend, and rejects the switch
    // when none is selected (see endpoints/v1/active_model.md).
    let source = data
        .and_then(|d| d.get("source"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    Ok(Command::SetModel {
        model: model_str.to_string(),
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

/// The `model` a per-model device command addresses. Required and non-empty:
/// a device belongs to a model, and there is deliberately no "the current one"
/// fallback because the command must work for a model that is not loaded —
/// staging a device before the first load is the main thing these verbs are
/// for. Unlike the per-model *language* commands, `source` is not carried:
/// the model is resolved against the active backend, which is the only backend
/// whose model could be the loaded one.
fn model_device_target(request: &DaemonRequest, command: &str) -> Result<String, String> {
    let model = request
        .data
        .as_ref()
        .and_then(|d| d.get("model"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if model.is_empty() {
        return Err(format!("Missing model for {command} command"));
    }
    if let Err(e) =
        validation::validate_string(&model, "model", validation::limits::MAX_NAME_LENGTH)
    {
        return Err(e.to_string());
    }
    Ok(model)
}

/// The `device` a per-model device setter carries. Only presence and length
/// are checked here; the daemon validates the value itself (`cpu`/`gpu` and
/// the deprecated spellings) so it can answer with the documented
/// `invalid_device` code rather than a bare parse error.
fn model_device_value(request: &DaemonRequest, command: &str) -> Result<String, String> {
    let device = request
        .data
        .as_ref()
        .and_then(|d| d.get("device"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("Missing device for {command} command"))?
        .to_string();
    if let Err(e) =
        validation::validate_string(&device, "device", validation::limits::MAX_NAME_LENGTH)
    {
        return Err(e.to_string());
    }
    Ok(device)
}

fn cmd_set_model_device(request: &DaemonRequest) -> Result<Command, String> {
    let model = model_device_target(request, "set_model_device")?;
    let device = model_device_value(request, "set_model_device")?;
    Ok(Command::SetModelDevice { model, device })
}

fn cmd_get_model_device(request: &DaemonRequest) -> Result<Command, String> {
    let model = model_device_target(request, "get_model_device")?;
    Ok(Command::GetModelDevice { model })
}

fn cmd_set_notification_method(request: &DaemonRequest) -> Result<Command, String> {
    let method = request
        .data
        .as_ref()
        .and_then(|data| data.get("method"))
        .and_then(|v| v.as_str())
        .ok_or("Missing method for set_notification_method command")?
        .to_string();

    Ok(Command::SetNotificationMethod { method })
}

fn cmd_set_allow_online_models(request: &DaemonRequest) -> Result<Command, String> {
    let enabled = request
        .enabled
        .ok_or("Missing enabled field for set_allow_online_models command")?;
    Ok(Command::SetAllowOnlineModels { enabled })
}

fn cmd_set_update_check_enabled(request: &DaemonRequest) -> Result<Command, String> {
    let enabled = request
        .enabled
        .ok_or("Missing enabled field for set_update_check_enabled command")?;
    Ok(Command::SetUpdateCheckEnabled { enabled })
}

fn cmd_set_update_beta_optin(request: &DaemonRequest) -> Result<Command, String> {
    let value = request
        .data
        .as_ref()
        .and_then(|data| data.get("value"))
        .and_then(|v| v.as_str())
        .ok_or("Missing value for set_update_beta_optin command")?
        .to_string();

    Ok(Command::SetUpdateBetaOptin { value })
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

/// Extract the `(source, model)` pair every per-model setting command carries
/// in `data` — language and voice alike. Both are required.
fn model_setting_target(
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
    let (source, model) = model_setting_target(request, "set_model_language")?;
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
    let (source, model) = model_setting_target(request, "get_model_language")?;
    Ok(Command::GetModelLanguage { source, model })
}

fn cmd_clear_model_language(request: &DaemonRequest) -> Result<Command, String> {
    let (source, model) = model_setting_target(request, "clear_model_language")?;
    Ok(Command::ClearModelLanguage { source, model })
}

/// The listing verb carries the same `(source, model)` pair its three siblings
/// do, extracted by the same helper. Sharing it is what stops the read verb
/// from accepting a request the write verbs reject (or the reverse): a client
/// that can fill a picker for a model must be able to write to that same
/// model, and a divergence here would be visible only as a picker whose
/// choices all fail on submit.
fn cmd_list_model_languages(request: &DaemonRequest) -> Result<Command, String> {
    let (source, model) = model_setting_target(request, "list_model_languages")?;
    Ok(Command::ListModelLanguages { source, model })
}

/// The four voice verbs address a model exactly as the four language verbs do,
/// through the same helper, so a client that can read one preference can write
/// the other without learning a second addressing scheme.
fn cmd_set_model_voice(request: &DaemonRequest) -> Result<Command, String> {
    let (source, model) = model_setting_target(request, "set_model_voice")?;
    let voice = request
        .data
        .as_ref()
        .and_then(|data| data.get("voice"))
        .and_then(|v| v.as_str())
        .ok_or("Missing voice for set_model_voice command")?
        .to_string();
    Ok(Command::SetModelVoice {
        source,
        model,
        voice,
    })
}

fn cmd_get_model_voice(request: &DaemonRequest) -> Result<Command, String> {
    let (source, model) = model_setting_target(request, "get_model_voice")?;
    Ok(Command::GetModelVoice { source, model })
}

fn cmd_clear_model_voice(request: &DaemonRequest) -> Result<Command, String> {
    let (source, model) = model_setting_target(request, "clear_model_voice")?;
    Ok(Command::ClearModelVoice { source, model })
}

fn cmd_list_model_voices(request: &DaemonRequest) -> Result<Command, String> {
    let (source, model) = model_setting_target(request, "list_model_voices")?;
    Ok(Command::ListModelVoices { source, model })
}
