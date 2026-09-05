// SPDX-License-Identifier: GPL-3.0-only

//! Shared `Task` builders re-used across handlers, so the same daemon call +
//! result-to-`Message` mapping isn't re-rolled at each call site.

use crate::daemon::client::{
    get_current_audio_theme, get_custom_models_dir, get_notification_method, get_preview_typing,
    get_recording_stop_mode, get_stage_view, get_update_check_enabled, get_update_status,
    get_volume, get_write_method, list_backends, ping_daemon,
};
use crate::state::AudioTheme;
use crate::state::device_offers::PP_STAGE;
use crate::ui::messages::{
    BackendMessage, DaemonMessage, Message, ModelsPageMessage, NotificationMethodMessage,
    PostProcessorMessage, PreviewTypingMessage, RecordingStopModeMessage, UpdateMessage,
    WriteMethodMessage,
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

/// Reload both backend catalogs: the installed list and the annotated registry.
///
/// They carry different halves of one picture — `/backend/list` names what is
/// installed (and its version), `/registry/backend/list` annotates each entry with
/// the `installed_version` it reads off disk per request — and every question a
/// card asks needs both. Refreshed apart, a card can report a version from one
/// and an update from the other and have them disagree.
///
/// No index refresh: the annotation is computed from local state, so the cached
/// index is enough and a network round-trip would only make this slower than
/// the moment that needs it.
pub(in crate::core::app) fn reload_backend_catalogs() -> Task<cosmic::Action<Message>> {
    Task::batch([reload_backends(), fetch_registry_catalog(false)])
}

/// Reload the installed-backend catalog and map the result to `BackendsLoaded`
/// / `BackendsError`.
pub(in crate::core::app) fn reload_backends() -> Task<cosmic::Action<Message>> {
    Task::batch([
        Task::perform(list_backends(), |result| {
            cosmic::Action::App(match result {
                Ok(backends) => Message::Backend(BackendMessage::BackendsLoaded(backends)),
                Err(e) => Message::Backend(BackendMessage::BackendsError(e.to_string())),
            })
        }),
        // Which of them can fill each stage is the daemon's answer, not a
        // filter of the catalog above — and it changes for exactly the same
        // reasons, so it is re-asked here rather than anywhere else.
        crate::core::app::AppModel::load_stage_backends(),
    ])
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
        load_post_processor(),
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
        Task::perform(get_notification_method(), |result| {
            use super_stt_shared::models::notification_method::NotificationMethod;
            match result {
                Ok(method_str) => {
                    let method = method_str.parse::<NotificationMethod>().unwrap_or_default();
                    cosmic::Action::App(Message::NotificationMethod(
                        NotificationMethodMessage::Loaded(method),
                    ))
                }
                Err(e) => {
                    warn!("Failed to load notification method: {e}");
                    cosmic::Action::App(Message::NotificationMethod(
                        NotificationMethodMessage::Loaded(NotificationMethod::default()),
                    ))
                }
            }
        }),
        refresh_update_status(),
        load_update_check_enabled(),
    ])
}

/// Read the post-processor selection. Part of the startup settings load, and
/// re-run by the `SettingsChanged { setting: "post_processor" }` SSE handler
/// when another client changes it.
///
/// A read failure — including a daemon predating the endpoint — leaves the
/// section at its default (off, nothing selected) rather than failing the whole
/// settings load, like every other getter in the batch.
pub(in crate::core::app) fn load_post_processor() -> Task<cosmic::Action<Message>> {
    Task::perform(get_stage_view(PP_STAGE), |result| match result {
        Ok(state) => {
            cosmic::Action::App(Message::PostProcessor(PostProcessorMessage::Loaded(state)))
        }
        Err(e) => {
            log::warn!("Failed to load the post-processor setting: {e}");
            cosmic::Action::App(Message::PostProcessor(PostProcessorMessage::Loaded(
                crate::daemon::client::StageState::default(),
            )))
        }
    })
}

/// Re-fetch the self-update status. Used by connection-time settings loads
/// (`build_load_settings_tasks`), by `on_nav_select` when the Updates page is
/// opened, and by the `SettingsChanged { setting }` SSE handler for
/// `update_check_enabled`/`update_beta_optin` (the latter because
/// `beta_optin_effective` rides this status, not a separate field), so its
/// data isn't stale from whenever the app last connected or last re-fetched.
pub(in crate::core::app) fn refresh_update_status() -> Task<cosmic::Action<Message>> {
    Task::perform(get_update_status(), |result| match result {
        Ok(status) => cosmic::Action::App(Message::Update(UpdateMessage::StatusLoaded(status))),
        Err(e) => cosmic::Action::App(Message::Update(UpdateMessage::StatusError(e.to_string()))),
    })
}

/// Load the automatic-check-enabled setting, defaulting to `true` (its
/// documented daemon-side default) on a fetch failure rather than leaving
/// the Updates page's toggler in a misleading off state. Also re-used by the
/// `SettingsChanged { setting: "update_check_enabled" }` SSE handler so a
/// change from another client is reflected here too.
pub(in crate::core::app) fn load_update_check_enabled() -> Task<cosmic::Action<Message>> {
    Task::perform(get_update_check_enabled(), |result| match result {
        Ok(enabled) => {
            cosmic::Action::App(Message::Update(UpdateMessage::AutoCheckLoaded(enabled)))
        }
        Err(e) => {
            warn!("Failed to load update-check-enabled setting: {e}");
            cosmic::Action::App(Message::Update(UpdateMessage::AutoCheckLoaded(true)))
        }
    })
}
