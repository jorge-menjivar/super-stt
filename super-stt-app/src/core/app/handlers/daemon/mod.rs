// SPDX-License-Identifier: GPL-3.0-only

mod events;

use crate::core::app::events::classify_daemon_error;
use crate::core::app::subscription::SETTINGS_APP_ID;
use crate::core::app::{AppModel, DeviceState, ModelOperationState};
use crate::daemon::client::{
    get_current_audio_theme, get_custom_models_dir, get_preview_typing, get_recording_stop_mode,
    get_volume, get_write_method, ping_daemon, test_daemon_connection,
};
use crate::state::{AudioTheme, DaemonStatus, Page};
use crate::ui::messages::Message;
use cosmic::prelude::*;
use log::{info, warn};

impl AppModel {
    /// Handle daemon connection messages
    pub(in crate::core::app) fn handle_daemon_messages(
        &mut self,
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            Message::ConnectToDaemon
            | Message::DaemonConnectionResult(_)
            | Message::RefreshDaemonStatus
            | Message::PingTimeout => self.handle_daemon_connect_result(message),

            Message::DaemonConnected => self.handle_daemon_connected(),

            Message::DaemonError(_)
            | Message::RetryConnection
            | Message::WidgetBlocked(_)
            | Message::RetryAuthorization => self.handle_daemon_errors_retry(message),

            Message::CurrentAudioThemeLoaded(_)
            | Message::VolumeLoaded(_)
            | Message::CustomModelsDirLoaded(_) => self.handle_daemon_initial_loads(message),

            Message::DaemonEventsReceived(_) | Message::DaemonEventsError(_) => {
                self.handle_daemon_events(message)
            }

            _ => Task::none(),
        }
    }

    fn handle_daemon_connect_result(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::ConnectToDaemon => {
                self.daemon_status = DaemonStatus::Connecting;
                Task::perform(test_daemon_connection(), |result| {
                    cosmic::Action::App(Message::DaemonConnectionResult(result))
                })
            }

            Message::DaemonConnectionResult(result) => {
                match result {
                    Ok(()) => {
                        self.daemon_status = DaemonStatus::Connected;
                    }
                    Err(e) => {
                        self.daemon_status = classify_daemon_error(e);
                    }
                }
                Task::none()
            }

            Message::RefreshDaemonStatus => Task::perform(test_daemon_connection(), |result| {
                cosmic::Action::App(Message::DaemonConnectionResult(result))
            }),

            Message::PingTimeout => {
                // Surface a stalled model switch (no progress for too long)
                // rather than letting the UI spin on the now-untimed POST.
                self.check_switch_stall();
                if self.daemon_status == DaemonStatus::Connected {
                    Task::perform(ping_daemon(), |result| {
                        cosmic::Action::App(match result {
                            Ok(_) => Message::DaemonConnected,
                            Err(e) => Message::DaemonError(e),
                        })
                    })
                } else {
                    Task::none()
                }
            }

            _ => Task::none(),
        }
    }

    fn handle_daemon_connected(&mut self) -> Task<cosmic::Action<Message>> {
        // Only switch to Settings page if we're transitioning from disconnected to connected
        let was_disconnected = self.daemon_status != DaemonStatus::Connected;

        self.daemon_status = DaemonStatus::Connected;
        // Only clear potentially stuck switching states on actual reconnect, not periodic pings
        if was_disconnected {
            self.device_state = DeviceState::Ready;
            self.model_operation_state = ModelOperationState::Ready;
            self.transcription_text.clear();
        }

        // The /events subscription is self-healing
        // (`run_widget_subscription` in super-stt-shared owns
        // its own reconnect loop), so we deliberately do NOT
        // bump `udp_restart_counter` on reconnect. Restarting
        // the iced subscription would cancel the helper
        // mid-retry and cause another `session::obtain` round
        // — i.e. another potential keyring touch (session-token reload).
        if was_disconnected {
            info!("Daemon reconnected; events subscription is self-healing, no iced restart");
        }

        // Only switch to first page on initial connection, not on periodic pings
        if was_disconnected {
            let mut first_entity = None;
            for entity in self.nav.iter() {
                if matches!(self.nav.data::<Page>(entity), Some(Page::Customization)) {
                    first_entity = Some(entity);
                    break;
                }
            }
            if let Some(entity) = first_entity {
                self.nav.activate(entity);
            }
        }

        let load_settings = build_load_settings_tasks();
        let load_primary_language = self.load_primary_language();

        if was_disconnected {
            Task::batch([
                self.handle_model_messages(Message::LoadInitialData),
                load_settings,
                load_primary_language,
            ])
        } else {
            Task::batch([load_settings, load_primary_language])
        }
    }

    fn handle_daemon_errors_retry(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::DaemonError(err) => {
                // Route user-denied responses into the Blocked state
                // so we don't keep pinging the daemon every 5s and
                // re-priming its in-memory deny cache. Any other
                // error is transient (daemon restarting, socket
                // missing, etc.) and gets the auto-retry loop.
                let next = classify_daemon_error(err);
                if matches!(next, DaemonStatus::Blocked(_)) {
                    warn!("Daemon access blocked by user denial; auto-retry suppressed: {next:?}");
                    self.daemon_status = next;
                    Task::none()
                } else {
                    self.daemon_status = next;
                    Task::perform(
                        async {
                            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        },
                        |()| cosmic::Action::App(Message::RetryConnection),
                    )
                }
            }

            Message::RetryConnection => {
                self.daemon_status = DaemonStatus::Connecting;
                Task::perform(ping_daemon(), |result| {
                    cosmic::Action::App(match result {
                        Ok(_) => Message::DaemonConnected,
                        Err(e) => Message::DaemonError(e),
                    })
                })
            }

            Message::WidgetBlocked(reason) => {
                warn!("Widget subscription blocked ({reason}); halting auto-retry");
                self.daemon_status = DaemonStatus::Blocked(reason);
                Task::none()
            }

            Message::RetryAuthorization => {
                info!("Retrying authorization after user denial");
                // Drop the cached settings token so the next
                // session::obtain fires /auth/request and (after a
                // daemon restart) spawns a fresh consent popup.
                if let Err(e) = super_stt_shared::daemon::session::forget(SETTINGS_APP_ID) {
                    warn!("Failed to forget settings session before retry: {e}");
                }
                // Bump the iced subscription key so the audio-events
                // task re-spawns from scratch — its previous instance
                // ended with `Blocked`.
                self.udp_restart_counter = self.udp_restart_counter.wrapping_add(1);
                self.daemon_status = DaemonStatus::Connecting;
                Task::perform(ping_daemon(), |result| {
                    cosmic::Action::App(match result {
                        Ok(_) => Message::DaemonConnected,
                        Err(e) => Message::DaemonError(e),
                    })
                })
            }

            _ => Task::none(),
        }
    }

    fn handle_daemon_initial_loads(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::CurrentAudioThemeLoaded(theme) => {
                self.selected_audio_theme = theme;
                if theme != AudioTheme::Silent {
                    self.last_non_silent_theme = theme;
                }
                Task::none()
            }

            Message::VolumeLoaded(vol) => {
                self.volume = vol;
                Task::none()
            }

            Message::CustomModelsDirLoaded(custom_path) => {
                let old_committed = self.custom_models_dir.as_deref().unwrap_or_default();
                if self.custom_models_dir_input == old_committed {
                    self.custom_models_dir_input =
                        custom_path.as_deref().unwrap_or_default().to_string();
                }
                self.custom_models_dir = custom_path;
                Task::none()
            }

            _ => Task::none(),
        }
    }
}

// Reload models, device info, and per-setting state on reconnect.
// Each setting is fetched with its own dedicated GET call —
// no bulk fetch_daemon_config anymore.
pub(in crate::core::app) fn build_load_settings_tasks() -> Task<cosmic::Action<Message>> {
    Task::batch([
        Task::perform(get_current_audio_theme(), |result| match result {
            Ok(theme) => cosmic::Action::App(Message::CurrentAudioThemeLoaded(theme)),
            Err(e) => {
                warn!("Failed to load audio theme: {e}");
                cosmic::Action::App(Message::CurrentAudioThemeLoaded(AudioTheme::default()))
            }
        }),
        Task::perform(get_volume(), |result| match result {
            Ok(vol) => cosmic::Action::App(Message::VolumeLoaded(vol)),
            Err(e) => {
                warn!("Failed to load volume: {e}");
                cosmic::Action::App(Message::VolumeLoaded(100))
            }
        }),
        Task::perform(get_custom_models_dir(), |result| match result {
            Ok(dir) => cosmic::Action::App(Message::CustomModelsDirLoaded(dir)),
            Err(e) => {
                warn!("Failed to load custom models dir: {e}");
                cosmic::Action::App(Message::CustomModelsDirLoaded(None))
            }
        }),
        Task::perform(get_preview_typing(), |result| match result {
            Ok(enabled) => cosmic::Action::App(Message::PreviewTypingSettingLoaded(enabled)),
            Err(e) => {
                log::warn!("Failed to load preview typing setting: {e}");
                cosmic::Action::App(Message::PreviewTypingSettingLoaded(false))
            }
        }),
        Task::perform(get_recording_stop_mode(), |result| {
            use super_stt_shared::models::recording_stop_mode::RecordingStopMode;
            match result {
                Ok(mode_str) => {
                    let mode = mode_str.parse::<RecordingStopMode>().unwrap_or_default();
                    cosmic::Action::App(Message::RecordingStopModeLoaded(mode))
                }
                Err(e) => {
                    log::warn!("Failed to load recording stop mode: {e}");
                    cosmic::Action::App(Message::RecordingStopModeLoaded(
                        RecordingStopMode::default(),
                    ))
                }
            }
        }),
        Task::perform(get_write_method(), |result| {
            use super_stt_shared::models::write_method::WriteMethod;
            match result {
                Ok(method_str) => {
                    let method = method_str.parse::<WriteMethod>().unwrap_or_default();
                    cosmic::Action::App(Message::WriteMethodLoaded(method))
                }
                Err(e) => {
                    log::warn!("Failed to load write method: {e}");
                    cosmic::Action::App(Message::WriteMethodLoaded(WriteMethod::default()))
                }
            }
        }),
    ])
}
