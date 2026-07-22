// SPDX-License-Identifier: GPL-3.0-only
use std::time::Instant;

use cosmic::{
    app as cosmic_app,
    iced::window,
    surface::{action as surface_action, surface_task},
    widget::segmented_button::Entity,
};
use log::{debug, info, warn};

use super::SuperSttApplet;
use crate::app::Message;
use crate::daemon::identity::APP_ID;
use crate::daemon::{RetryStrategy, ping_daemon};
use crate::models::state::{DaemonConnectionState, IsOpen, RecordingState};
use crate::models::theme::{
    IconAlignment, VisualizationColor, VisualizationTheme, WorkingAnimationTheme,
};
use super_stt_shared::daemon::session;

impl SuperSttApplet {
    pub(super) fn handle_message(&mut self, message: Message) -> cosmic_app::Task<Message> {
        match message {
            Message::TogglePopup => self.toggle_popup(),
            Message::CloseRequested(id) => self.close_popup(id),
            Message::DaemonConnected => self.daemon_connected(),
            Message::PingResponse => self.ping_response(),
            Message::DaemonError(err) => self.daemon_error(&err),
            Message::ScheduleRetry => self.schedule_retry(),
            Message::RevealerToggle(src) => self.revealer_toggle(src),
            Message::SetVisualizationTheme(theme) => self.set_visualization_theme(theme),
            Message::SetWorkingAnimation(theme) => self.set_working_animation(theme),
            Message::WidgetRecordingState(is_recording) => {
                self.widget_recording_state(is_recording)
            }
            Message::WidgetFrequencyBands {
                bands,
                total_energy,
            } => self.widget_frequency_bands(&bands, total_energy),
            Message::WidgetTranscribingStarted => {
                // Guard against out-of-order delivery: only enter Processing
                // mid-cycle, never resurrect it after transcribing_stopped
                // already returned us to Idle.
                if !matches!(self.recording_state, RecordingState::Idle) {
                    self.set_recording_state(RecordingState::Processing);
                }
                cosmic_app::Task::none()
            }
            Message::WidgetTranscribingStopped => {
                self.set_recording_state(RecordingState::Idle);
                self.visualization.clear();
                cosmic_app::Task::none()
            }
            Message::WidgetRevoked(reason) => self.widget_revoked(&reason),
            Message::WidgetOtherEvent(_) | Message::WidgetSubscriptionError(_) => {
                // Informational only; subscription errors are logged in
                // the subscription task itself.
                cosmic_app::Task::none()
            }
            Message::WidgetBlocked(reason) => self.widget_blocked(reason),
            Message::RetryAuthorization => self.retry_authorization(),
            Message::RetryConnection => self.retry_connection(),
            Message::PingTimeout => self.ping_timeout(),
            Message::OpenGitHub => Self::open_github(),
            Message::LaunchApp => Self::launch_app(),
            Message::SetAppletWidth(width) => self.set_applet_width(width),
            Message::SetShowIcon(show_icon) => self.set_show_icon(show_icon),
            Message::SetIconAlignmentEntity(entity) => self.set_icon_alignment(entity),
            Message::SetShowVisualizations(show) => self.set_show_visualizations(show),
            Message::SetVisualizationColor(color, is_dark) => {
                self.set_visualization_color(color, is_dark)
            }
            Message::SetColorThemeEntity(entity) => self.set_color_theme(entity),
            Message::WorkingAnimationTick => {
                if let Some(start) = self.working_anim_start {
                    self.working_animation
                        .set_elapsed(start.elapsed().as_secs_f32() * 1000.0);
                }
                cosmic_app::Task::none()
            }
        }
    }

    /// Open or close the panel popup.
    ///
    /// Routed through `cosmic::surface` rather than the raw `get_popup` /
    /// `destroy_popup` wayland commands so libcosmic tracks the surface: it
    /// owns the popup's corner radii and its frosted-glass blur, and re-applies
    /// both when the theme changes. A popup spawned directly is invisible to
    /// that bookkeeping, which leaves it translucent with nothing blurred
    /// behind it whenever the theme has frosted applets on.
    fn toggle_popup(&mut self) -> cosmic_app::Task<Message> {
        if let Some(p) = self.popup.take() {
            return surface_task(surface_action::destroy_popup(p));
        }
        let Some(main_window_id) = self.core.main_window_id() else {
            warn!("Cannot toggle popup: main window ID not available");
            return cosmic_app::Task::none();
        };
        surface_task(surface_action::app_popup::<Self>(
            // Defaults: inherit the blur and corner radii libcosmic derives
            // from the theme for an applet popup.
            |_| surface_action::LiveSettings::default(),
            move |app: &mut Self| {
                let new_id = window::Id::unique();
                app.popup.replace(new_id);
                app.core
                    .applet
                    .get_popup_settings(main_window_id, new_id, None, None, None)
            },
            // No dedicated view: libcosmic falls back to `view_window`.
            None,
        ))
    }

    fn close_popup(&mut self, id: window::Id) -> cosmic_app::Task<Message> {
        if Some(id) == self.popup {
            self.popup = None;
        }
        cosmic_app::Task::none()
    }

    fn daemon_connected(&mut self) -> cosmic_app::Task<Message> {
        // Log the connect only on the actual transition — this fires from both
        // a successful (re)connect ping and the subscription's `Connected`
        // update, which would otherwise double-log at startup.
        if self.daemon_state != DaemonConnectionState::Connected {
            info!("Connected to daemon");
        }
        self.daemon_state = DaemonConnectionState::Connected;
        self.retry_strategy.reset();
        // The widget /events subscription is self-healing
        // (`run_widget_subscription` in super-stt-shared owns its own
        // reconnect loop), so we deliberately do NOT bump
        // `udp_restart_counter` here. Restarting the iced subscription
        // on every successful ping would cancel the helper task
        // mid-flight and force every ping cycle to re-enter
        // `session::obtain` — i.e. another potential keyring touch.
        cosmic_app::Task::none()
    }

    fn ping_response(&mut self) -> cosmic_app::Task<Message> {
        // Fires on every periodic liveness ping (~5 s) while connected — pure
        // steady state, so log at debug. A successful HTTP `/ping` means the
        // daemon is reachable; keep the connection live and reset backoff.
        debug!("Daemon ping OK");
        self.daemon_state = DaemonConnectionState::Connected;
        self.retry_strategy.reset();
        cosmic_app::Task::none()
    }

    fn daemon_error(&mut self, err: &str) -> cosmic_app::Task<Message> {
        warn!("Daemon error: {err}");
        // Reset backoff when an established connection drops so reconnect
        // starts from the initial-connection strategy.
        if matches!(self.daemon_state, DaemonConnectionState::Connected) {
            self.retry_strategy = RetryStrategy::for_initial_connection();
            info!("Lost connection to daemon, starting reconnection attempts");
        }
        // Retry forever — schedule the next attempt.
        cosmic_app::Task::perform(async {}, |()| cosmic::Action::App(Message::ScheduleRetry))
    }

    fn schedule_retry(&mut self) -> cosmic_app::Task<Message> {
        self.retry_strategy.should_retry(); // Always true; increments the attempt counter.
        let delay = self.retry_strategy.next_delay();
        debug!(
            "Scheduling daemon connection retry {} in {:?}",
            self.retry_strategy.attempt, delay
        );
        self.daemon_state = DaemonConnectionState::Connecting;
        cosmic_app::Task::perform(
            async move {
                tokio::time::sleep(delay).await;
            },
            |()| cosmic::Action::App(Message::RetryConnection),
        )
    }

    fn revealer_toggle(&mut self, is_open_src: IsOpen) -> cosmic_app::Task<Message> {
        self.is_open = if self.is_open == is_open_src {
            IsOpen::None
        } else {
            is_open_src
        };
        cosmic_app::Task::none()
    }

    fn set_visualization_theme(&mut self, theme: VisualizationTheme) -> cosmic_app::Task<Message> {
        self.config
            .update(|c| c.visualization.theme = theme.clone());
        self.visualization.update_theme(theme);
        self.visualization
            .update_audio_level(self.audio_level, self.is_speech_detected);
        self.is_open = IsOpen::None;
        cosmic_app::Task::none()
    }

    fn set_working_animation(&mut self, theme: WorkingAnimationTheme) -> cosmic_app::Task<Message> {
        self.config
            .update(|c| c.visualization.working_animation = theme);
        self.working_animation.update_theme(theme);
        self.is_open = IsOpen::None;
        cosmic_app::Task::none()
    }

    /// Set the recording state, managing the working-animation clock: start it
    /// when entering Processing, stop it otherwise. Centralizes the lifecycle
    /// so every transition keeps the animation in sync.
    fn set_recording_state(&mut self, new: RecordingState) {
        if matches!(new, RecordingState::Processing) {
            if self.working_anim_start.is_none() {
                self.working_anim_start = Some(Instant::now());
                self.working_animation.reset();
            }
        } else {
            self.working_anim_start = None;
        }
        self.recording_state = new;
    }

    fn widget_recording_state(&mut self, is_recording: bool) -> cosmic_app::Task<Message> {
        let was_recording = matches!(self.recording_state, RecordingState::Recording);
        let new_state = if is_recording {
            RecordingState::Recording
        } else if was_recording {
            // Just left Recording — show a brief Processing state while
            // the daemon transcribes.
            RecordingState::Processing
        } else {
            RecordingState::Idle
        };

        self.set_recording_state(new_state);
        if was_recording && !is_recording {
            self.visualization.clear();
        }
        cosmic_app::Task::none()
    }

    fn widget_frequency_bands(
        &mut self,
        bands: &[f32],
        total_energy: f32,
    ) -> cosmic_app::Task<Message> {
        self.visualization
            .update_frequency_bands(bands, total_energy);
        self.audio_level = total_energy;
        self.is_speech_detected = total_energy > 0.02;
        cosmic_app::Task::none()
    }

    fn widget_revoked(&mut self, reason: &str) -> cosmic_app::Task<Message> {
        warn!(
            "Widget session revoked by daemon (reason={reason}); will re-subscribe via consent flow on next retry"
        );
        // Treat like a connection drop so the existing reconnect path
        // triggers a fresh /auth/request → consent popup → re-subscribe.
        self.daemon_state = DaemonConnectionState::Error(format!("revoked: {reason}"));
        cosmic_app::Task::none()
    }

    fn widget_blocked(&mut self, reason: String) -> cosmic_app::Task<Message> {
        warn!("Widget subscription blocked by user denial ({reason}); waiting for explicit retry");
        self.daemon_state = DaemonConnectionState::Blocked(reason);
        cosmic_app::Task::none()
    }

    fn retry_authorization(&mut self) -> cosmic_app::Task<Message> {
        info!("Retrying authorization after user denial");
        // Drop any cached token (in-memory + keyring) so the next
        // subscription cycle hits the daemon's /auth/request and spawns
        // a fresh consent prompt.
        if let Err(e) = session::forget(APP_ID) {
            warn!("Failed to forget session before retry: {e}");
        }
        // Restart the iced subscription so the dropped helper task
        // re-spawns from scratch.
        self.udp_restart_counter = self.udp_restart_counter.wrapping_add(1);
        self.daemon_state = DaemonConnectionState::Connecting;
        self.retry_strategy = RetryStrategy::for_initial_connection();
        cosmic_app::Task::none()
    }

    fn retry_connection(&mut self) -> cosmic_app::Task<Message> {
        if matches!(self.daemon_state, DaemonConnectionState::Error(_)) {
            // Manual retry from the error state — reset backoff.
            self.retry_strategy = RetryStrategy::for_initial_connection();
            self.daemon_state = DaemonConnectionState::Connecting;
            info!("Manual retry initiated by user");
        }
        debug!(
            "Retrying daemon connection (attempt {})...",
            self.retry_strategy.attempt
        );
        cosmic_app::Task::perform(ping_daemon(self.socket_path.clone()), |result| {
            cosmic::Action::App(match result {
                Ok(_) => Message::DaemonConnected,
                Err(e) => {
                    debug!("Retry failed: {e}");
                    Message::ScheduleRetry
                }
            })
        })
    }

    fn ping_timeout(&mut self) -> cosmic_app::Task<Message> {
        if self.daemon_state == DaemonConnectionState::Connected {
            return cosmic_app::Task::perform(ping_daemon(self.socket_path.clone()), |result| {
                cosmic::Action::App(match result {
                    Ok(_) => Message::PingResponse,
                    Err(e) => {
                        warn!("Daemon ping failed: {e}");
                        Message::DaemonError(format!("Connection lost: {e}"))
                    }
                })
            });
        }
        if self.daemon_state == DaemonConnectionState::Connecting && self.retry_strategy.attempt > 5
        {
            // The retry strategy already drives reconnection during the
            // initial connect; just log occasionally.
            debug!(
                "Still attempting to connect (attempt {})...",
                self.retry_strategy.attempt
            );
        }
        cosmic_app::Task::none()
    }

    /// Spawn a fire-and-forget child and reap it in a detached thread, so a
    /// launched helper never lingers as a zombie in the session-long applet
    /// (Tier 1 #22). We don't care about the exit status — the `wait` only
    /// prevents the zombie.
    fn spawn_detached(cmd: &mut std::process::Command) -> std::io::Result<()> {
        let mut child = cmd.spawn()?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    fn open_github() -> cosmic_app::Task<Message> {
        if let Err(e) =
            Self::spawn_detached(std::process::Command::new("xdg-open").arg(crate::REPOSITORY))
        {
            warn!("Failed to open GitHub URL: {e}");
        }
        cosmic_app::Task::none()
    }

    fn launch_app() -> cosmic_app::Task<Message> {
        // `Command::new("super-stt-app")` already searches `PATH`, then fall
        // back to the standard install prefixes. The old `./target/{debug,
        // release}` dev paths (resolved against the panel process' CWD) never
        // matched for an installed applet and were dropped (Tier 1 #22).
        let launch_attempts = [
            "super-stt-app",                // System PATH
            "/usr/local/bin/super-stt-app", // Local install
            "/usr/bin/super-stt-app",       // System install
        ];

        for command in launch_attempts {
            if Self::spawn_detached(&mut std::process::Command::new(command)).is_ok() {
                info!("Successfully launched Super STT app with command: {command}");
                return cosmic_app::Task::none();
            }
        }

        warn!("Failed to launch Super STT app - tried all common locations");
        cosmic_app::Task::none()
    }

    fn set_applet_width(&mut self, width: u32) -> cosmic_app::Task<Message> {
        self.config.update(|c| c.ui.applet_width = width);
        // Refresh the visualization so it adapts to the new size.
        self.visualization.clear();
        self.visualization
            .update_audio_level(self.audio_level, self.is_speech_detected);
        cosmic_app::Task::none()
    }

    fn set_show_icon(&mut self, show_icon: bool) -> cosmic_app::Task<Message> {
        self.config.update(|c| c.ui.show_icon = show_icon);
        cosmic_app::Task::none()
    }

    fn set_icon_alignment(&mut self, entity: Entity) -> cosmic_app::Task<Message> {
        self.icon_alignment_model.activate(entity);
        let alignment = if entity == self.icon_alignment_start {
            IconAlignment::Start
        } else if entity == self.icon_alignment_center {
            IconAlignment::Center
        } else if entity == self.icon_alignment_end {
            IconAlignment::End
        } else {
            IconAlignment::Start
        };
        self.config.update(|c| c.ui.icon_alignment = alignment);
        cosmic_app::Task::none()
    }

    fn set_show_visualizations(&mut self, show: bool) -> cosmic_app::Task<Message> {
        self.config.update(|c| c.ui.show_visualization = show);
        cosmic_app::Task::none()
    }

    fn set_visualization_color(
        &mut self,
        color: VisualizationColor,
        is_dark: bool,
    ) -> cosmic_app::Task<Message> {
        let mut updated_colors = self.config.visualization.colors.clone();
        updated_colors.set_color(color, is_dark);
        self.config
            .update(|c| c.visualization.colors = updated_colors.clone());
        self.working_animation.update_colors(updated_colors.clone());
        self.visualization.update_colors(updated_colors);
        cosmic_app::Task::none()
    }

    fn set_color_theme(&mut self, entity: Entity) -> cosmic_app::Task<Message> {
        self.theme_selector_model.activate(entity);
        if entity == self.theme_selector_light {
            self.selected_theme_for_config = false;
        } else if entity == self.theme_selector_dark {
            self.selected_theme_for_config = true;
        }
        cosmic_app::Task::none()
    }
}
