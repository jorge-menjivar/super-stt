// SPDX-License-Identifier: GPL-3.0-only

mod events;

use crate::core::app::events::classify_daemon_error;
use crate::core::app::handlers::tasks::{build_load_settings_tasks, ping_task};
use crate::core::app::subscription::SETTINGS_APP_ID;
use crate::core::app::{AppModel, DeviceState, ModelOperationState};
use crate::daemon::client::test_daemon_connection;
use crate::state::{AudioTheme, DaemonStatus};
use crate::ui::messages::{DaemonMessage, Message, ModelMessage};
use cosmic::prelude::*;
use log::{info, warn};

impl AppModel {
    /// Handle daemon connection messages
    pub(in crate::core::app) fn handle_daemon_messages(
        &mut self,
        message: DaemonMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            DaemonMessage::DaemonConnectionResult(_)
            | DaemonMessage::RefreshDaemonStatus
            | DaemonMessage::PingTimeout => self.handle_daemon_connect_result(message),

            DaemonMessage::DaemonConnected => self.handle_daemon_connected(),

            DaemonMessage::EventStreamConnected => {
                // Same connection-status handling as DaemonConnected, plus a
                // current-model re-fetch: the live event stream is now
                // subscribed, so re-read the daemon's authoritative model state
                // to capture anything that changed before this point (e.g. a
                // startup-load broadcast that fired before we subscribed). The
                // fetch is epoch-guarded, so it can't clobber a live event that
                // arrives while it's in flight.
                Task::batch([self.handle_daemon_connected(), self.fetch_current_model()])
            }

            DaemonMessage::DaemonError(_)
            | DaemonMessage::RetryConnection
            | DaemonMessage::WidgetBlocked(_)
            | DaemonMessage::RetryAuthorization => self.handle_daemon_errors_retry(message),

            DaemonMessage::CurrentAudioThemeLoaded(_)
            | DaemonMessage::VolumeLoaded(_)
            | DaemonMessage::CustomModelsDirLoaded(_) => self.handle_daemon_initial_loads(message),

            DaemonMessage::DaemonEventsReceived(_) => self.handle_daemon_events(message),
        }
    }

    fn handle_daemon_connect_result(
        &mut self,
        message: DaemonMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            DaemonMessage::DaemonConnectionResult(result) => {
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

            DaemonMessage::RefreshDaemonStatus => {
                Task::perform(test_daemon_connection(), |result| {
                    cosmic::Action::App(Message::Daemon(DaemonMessage::DaemonConnectionResult(
                        result,
                    )))
                })
            }

            DaemonMessage::PingTimeout => {
                // Surface a stalled model switch (no progress for too long)
                // rather than letting the UI spin on the now-untimed POST.
                self.check_switch_stall();
                if self.daemon_status == DaemonStatus::Connected {
                    ping_task()
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
        // Connected: reset the reconnect backoff so the next disconnect starts
        // from the short initial delay again.
        self.reconnect_retry.reset();
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

        // Do NOT navigate on (re)connect: the launch page is Models (set in
        // `init.rs`), and yanking a mid-flow user to another page on every
        // daemon restart was both surprising and contradicted that default
        // (Tier 1 #18). Whatever page the user is on stays active.

        // Load settings/models/languages ONLY on the disconnected→connected
        // transition. Periodic keep-alive pings also resolve to `DaemonConnected`
        // (daemon/mod.rs `PingTimeout`), so re-fetching here unconditionally re-ran
        // six settings GETs + a language load every tick and clobbered optimistic
        // local edits (Tier 1 #14). Cross-client settings sync rides the SSE
        // `settings_changed` topic, not this poll.
        if was_disconnected {
            // Fresh connection — drop any banner left over from before the drop.
            self.action_error = None;
            Task::batch([
                self.handle_model_messages(ModelMessage::LoadInitialData),
                build_load_settings_tasks(),
                self.load_primary_language(),
            ])
        } else {
            Task::none()
        }
    }

    fn handle_daemon_errors_retry(
        &mut self,
        message: DaemonMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            DaemonMessage::DaemonError(err) => {
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
                    // Exponential backoff with jitter (shared RetryStrategy)
                    // instead of a flat 5 s, so many clients reconnecting after a
                    // daemon restart don't stampede in lockstep.
                    let delay = self.reconnect_retry.next_delay();
                    self.reconnect_retry.should_retry();
                    Task::perform(
                        async move {
                            tokio::time::sleep(delay).await;
                        },
                        |()| cosmic::Action::App(Message::Daemon(DaemonMessage::RetryConnection)),
                    )
                }
            }

            DaemonMessage::RetryConnection => {
                self.daemon_status = DaemonStatus::Connecting;
                ping_task()
            }

            DaemonMessage::WidgetBlocked(reason) => {
                warn!("Widget subscription blocked ({reason}); halting auto-retry");
                self.daemon_status = DaemonStatus::Blocked(reason);
                Task::none()
            }

            DaemonMessage::RetryAuthorization => {
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
                ping_task()
            }

            _ => Task::none(),
        }
    }

    fn handle_daemon_initial_loads(
        &mut self,
        message: DaemonMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            DaemonMessage::CurrentAudioThemeLoaded(theme) => {
                self.selected_audio_theme = theme;
                if theme != AudioTheme::Silent {
                    self.last_non_silent_theme = theme;
                }
                Task::none()
            }

            DaemonMessage::VolumeLoaded(vol) => {
                self.volume = vol;
                Task::none()
            }

            DaemonMessage::CustomModelsDirLoaded(custom_path) => {
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
