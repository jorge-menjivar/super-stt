// SPDX-License-Identifier: GPL-3.0-only

//! Shared `Task` builders re-used across handlers, so the same daemon call +
//! result-to-`Message` mapping isn't re-rolled at each call site.

use crate::daemon::client::{
    get_current_audio_theme, get_custom_models_dir, get_preview_typing, get_recording_stop_mode,
    get_volume, get_write_method, list_backends, ping_daemon,
};
use crate::state::AudioTheme;
use crate::ui::messages::{
    BackendMessage, DaemonMessage, Message, ModelsPageMessage, PreviewTypingMessage,
    RecordingStopModeMessage, WriteMethodMessage,
};
use cosmic::prelude::*;
use log::warn;

/// Ping the daemon and map the result to `DaemonConnected` / `DaemonError`.
/// Shared by the periodic keep-alive, the reconnect retries, and the startup
/// ping.
pub(in crate::core::app) fn ping_task() -> Task<cosmic::Action<Message>> {
    Task::perform(ping_daemon(), |result| {
        cosmic::Action::App(Message::Daemon(match result {
            Ok(_) => DaemonMessage::DaemonConnected,
            Err(e) => DaemonMessage::DaemonError(e),
        }))
    })
}

/// Reload the installed-backend catalog and map the result to `BackendsLoaded`
/// / `BackendsError`.
pub(in crate::core::app) fn reload_backends() -> Task<cosmic::Action<Message>> {
    Task::perform(list_backends(), |result| {
        cosmic::Action::App(match result {
            Ok(backends) => Message::Backend(BackendMessage::BackendsLoaded(backends)),
            Err(e) => Message::Backend(BackendMessage::BackendsError(e.to_string())),
        })
    })
}

/// Fetch the full annotated registry catalog (optionally refreshing the index
/// first) and map the result to `RegistryListLoaded` / `RegistryListFailed`.
/// The catalog always includes incompatible entries; filtering is client-side.
pub(in crate::core::app) fn fetch_registry_catalog(refresh: bool) -> Task<cosmic::Action<Message>> {
    let filters = crate::daemon::registry::ListFilters {
        include_incompatible: Some(true),
        ..Default::default()
    };
    Task::perform(
        async move {
            if refresh {
                let _ = crate::daemon::registry::refresh().await;
            }
            crate::daemon::registry::list(&filters).await
        },
        |r| {
            cosmic::Action::App(Message::ModelsPage(match r {
                Ok(resp) => ModelsPageMessage::RegistryListLoaded(resp),
                Err(e) => ModelsPageMessage::RegistryListFailed(e.to_string()),
            }))
        },
    )
}

/// Reload models, device info, and per-setting state on reconnect. Each setting
/// is fetched with its own dedicated GET call — no bulk `fetch_daemon_config`.
pub(in crate::core::app) fn build_load_settings_tasks() -> Task<cosmic::Action<Message>> {
    Task::batch([
        Task::perform(get_current_audio_theme(), |result| match result {
            Ok(theme) => cosmic::Action::App(Message::Daemon(
                DaemonMessage::CurrentAudioThemeLoaded(theme),
            )),
            Err(e) => {
                warn!("Failed to load audio theme: {e}");
                cosmic::Action::App(Message::Daemon(DaemonMessage::CurrentAudioThemeLoaded(
                    AudioTheme::default(),
                )))
            }
        }),
        Task::perform(get_volume(), |result| match result {
            Ok(vol) => cosmic::Action::App(Message::Daemon(DaemonMessage::VolumeLoaded(vol))),
            Err(e) => {
                warn!("Failed to load volume: {e}");
                cosmic::Action::App(Message::Daemon(DaemonMessage::VolumeLoaded(100)))
            }
        }),
        Task::perform(get_custom_models_dir(), |result| match result {
            Ok(dir) => {
                cosmic::Action::App(Message::Daemon(DaemonMessage::CustomModelsDirLoaded(dir)))
            }
            Err(e) => {
                warn!("Failed to load custom models dir: {e}");
                cosmic::Action::App(Message::Daemon(DaemonMessage::CustomModelsDirLoaded(None)))
            }
        }),
        Task::perform(get_preview_typing(), |result| match result {
            Ok(enabled) => cosmic::Action::App(Message::PreviewTyping(
                PreviewTypingMessage::SettingLoaded(enabled),
            )),
            Err(e) => {
                log::warn!("Failed to load preview typing setting: {e}");
                cosmic::Action::App(Message::PreviewTyping(PreviewTypingMessage::SettingLoaded(
                    false,
                )))
            }
        }),
        Task::perform(get_recording_stop_mode(), |result| {
            use super_stt_shared::models::recording_stop_mode::RecordingStopMode;
            match result {
                Ok(mode_str) => {
                    let mode = mode_str.parse::<RecordingStopMode>().unwrap_or_default();
                    cosmic::Action::App(Message::RecordingStopMode(
                        RecordingStopModeMessage::Loaded(mode),
                    ))
                }
                Err(e) => {
                    log::warn!("Failed to load recording stop mode: {e}");
                    cosmic::Action::App(Message::RecordingStopMode(
                        RecordingStopModeMessage::Loaded(RecordingStopMode::default()),
                    ))
                }
            }
        }),
        Task::perform(get_write_method(), |result| {
            use super_stt_shared::models::write_method::WriteMethod;
            match result {
                Ok(method_str) => {
                    let method = method_str.parse::<WriteMethod>().unwrap_or_default();
                    cosmic::Action::App(Message::WriteMethod(WriteMethodMessage::Loaded(method)))
                }
                Err(e) => {
                    log::warn!("Failed to load write method: {e}");
                    cosmic::Action::App(Message::WriteMethod(WriteMethodMessage::Loaded(
                        WriteMethod::default(),
                    )))
                }
            }
        }),
    ])
}
