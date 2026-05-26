// SPDX-License-Identifier: GPL-3.0-only
mod app;
mod config;
mod daemon;
mod models;
mod ui;

use cosmic::{
    Element, app as cosmic_app,
    iced::{
        Alignment, Subscription,
        platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup},
        window,
    },
    iced_widget,
    theme::{self, Button},
    widget::{
        self, button, container, layer_container, mouse_area,
        segmented_button::{Entity, SingleSelectModel},
    },
};

use futures_util::{SinkExt, StreamExt};
use log::{info, warn};
use std::{path::PathBuf, rc::Rc};

// Cache icon bytes to avoid allocation on every render
static NORMAL_ICON: &[u8] = include_bytes!("../resources/assets/super-stt-icon.svg");
static TRANSPARENT_ICON: &[u8] = include_bytes!("../resources/assets/transparent-icon.svg");
static ERROR_ICON: &[u8] = include_bytes!("../resources/assets/error-icon.svg");

use crate::models::state::{DaemonConnectionState, RecordingState};
use crate::ui::components::sound_visualization::VisualizationComponent;
use crate::{app::Message, models::state::IsOpen};
use crate::{
    config::AppletConfig,
    ui::views::{PopupContentParams, create_popup_content},
};
use crate::{
    daemon::{RetryStrategy, ping_daemon, ping_daemon_with_status},
    models::theme::ThemeConfig,
};
use super_stt_shared::daemon::session::{self, AppId};
use super_stt_shared::daemon::widget_subscription::{
    WidgetSubscriptionConfig, WidgetSubscriptionUpdate, run_widget_subscription,
};
use super_stt_shared::validation::get_http_socket_path;

// Connection monitoring constants
const PING_INTERVAL_SECS: u64 = 5; // Ping every 5 seconds to check daemon health
const VISUALIZATION_HEIGHT: f32 = 100.0; // Visualization height in pixels

/// Wrapper for `Subscription::run_with` so the subscription restarts when the counter changes.
#[derive(Hash)]
struct UdpSubscriptionId(u64);

use cosmic::iced::{Length, Size};

// Export types needed by the binary files
pub use models::theme::VisualizationSide;

// Crate version sourced from Cargo.toml for UI display and CLI metadata
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// Run the Super STT COSMIC applet
///
/// # Errors
///
/// Returns an error if the applet fails to start or encounters
/// a runtime error during execution.
pub fn run() -> cosmic::iced::Result {
    cosmic::applet::run::<SuperSttApplet>(VisualizationSide::Full)
}

pub struct SuperSttApplet {
    core: cosmic::app::Core,
    recording_state: RecordingState,
    daemon_state: DaemonConnectionState,
    popup: Option<window::Id>,
    socket_path: PathBuf,
    audio_level: f32,
    is_speech_detected: bool,
    is_open: IsOpen,
    theme_config: ThemeConfig,
    udp_restart_counter: u64,
    visualization: VisualizationComponent,
    last_udp_data: std::time::Instant,
    config: AppletConfig,
    variant_name: String,
    icon_alignment_model: SingleSelectModel,
    icon_alignment_start: Entity,
    icon_alignment_center: Entity,
    icon_alignment_end: Entity,
    theme_selector_model: SingleSelectModel,
    theme_selector_light: Entity,
    theme_selector_dark: Entity,
    selected_theme_for_config: bool, // false = light, true = dark
    retry_strategy: RetryStrategy,
}

impl cosmic::Application for SuperSttApplet {
    type Message = Message;
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = VisualizationSide;
    const APP_ID: &'static str = "ai.menjivar.super-stt-cosmic-applet";

    fn init(
        core: cosmic::app::Core,
        visualization_side: Self::Flags,
    ) -> (Self, cosmic_app::Task<Self::Message>) {
        // Load persistent configuration
        let variant_name = AppletConfig::get_variant_name(&visualization_side).to_string();
        let config = AppletConfig::load(&variant_name, visualization_side.clone());

        // Create theme config from loaded configuration
        let theme_config = ThemeConfig {
            visualization_theme: config.visualization.theme.clone(),
            visualization_color_config: config.visualization.colors.clone(),
        };

        let visualization = VisualizationComponent::new(
            0.0,
            false,
            config.visualization.theme.clone(),
            visualization_side,
            config.visualization.colors.clone(),
        );

        // Initialize icon alignment model
        let mut icon_alignment_model = SingleSelectModel::default();
        let icon_alignment_start = icon_alignment_model.insert().text("Start").id();
        let icon_alignment_center = icon_alignment_model.insert().text("Center").id();
        let icon_alignment_end = icon_alignment_model.insert().text("End").id();

        // Set active alignment based on config
        match config.ui.icon_alignment.as_str() {
            "center" => icon_alignment_model.activate(icon_alignment_center),
            "end" => icon_alignment_model.activate(icon_alignment_end),
            _ => icon_alignment_model.activate(icon_alignment_start),
        }

        // Initialize theme selector model for color configuration
        let mut theme_selector_model = SingleSelectModel::default();
        let theme_selector_light = theme_selector_model.insert().text("Light Theme").id();
        let theme_selector_dark = theme_selector_model.insert().text("Dark Theme").id();

        // Default to current system theme for initial selection
        let current_theme = cosmic::theme::active();
        let is_dark = current_theme.cosmic().is_dark;
        let selected_theme_for_config = is_dark;

        if is_dark {
            theme_selector_model.activate(theme_selector_dark);
        } else {
            theme_selector_model.activate(theme_selector_light);
        }

        let applet = Self {
            core,
            recording_state: RecordingState::Idle,
            daemon_state: DaemonConnectionState::Connecting,
            popup: None,
            socket_path: super_stt_shared::validation::get_http_socket_path(),
            audio_level: 0.0,
            is_speech_detected: false,
            is_open: IsOpen::None,
            theme_config,
            udp_restart_counter: 0,
            visualization,
            last_udp_data: std::time::Instant::now(),
            config,
            variant_name,
            icon_alignment_model,
            icon_alignment_start,
            icon_alignment_center,
            icon_alignment_end,
            theme_selector_model,
            theme_selector_light,
            theme_selector_dark,
            selected_theme_for_config,
            retry_strategy: RetryStrategy::for_initial_connection(),
        };

        // Try to ping the daemon on startup
        let initial_ping =
            cosmic_app::Task::perform(ping_daemon(applet.socket_path.clone()), |result| {
                cosmic::Action::App(match result {
                    Ok(_) => Message::DaemonConnected,
                    Err(e) => {
                        info!("Initial daemon connection failed: {e}");
                        // Instead of immediately showing error, schedule a retry
                        Message::ScheduleRetry
                    }
                })
            });

        (applet, initial_ping)
    }

    fn core(&self) -> &cosmic::app::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::app::Core {
        &mut self.core
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            // Daemon `/events` SSE subscription. Wrapped in
            // `Subscription::run_with` keyed on `udp_restart_counter`
            // so a daemon-reconnect (`Message::DaemonConnected` bumps
            // the counter) tears down the old stream and starts a
            // fresh one with whatever auth state the daemon now has.
            // The name `UdpSubscriptionId` is preserved while the
            // legacy UDP path is being deprecated.
            Subscription::run_with(
                UdpSubscriptionId(self.udp_restart_counter),
                applet_events_subscription,
            ),
            // Periodic connection monitoring
            cosmic::iced::time::every(std::time::Duration::from_secs(PING_INTERVAL_SECS))
                .map(|_| Message::PingTimeout),
        ])
    }

    #[allow(clippy::too_many_lines)]
    fn update(&mut self, message: Self::Message) -> cosmic_app::Task<Self::Message> {
        match message {
            Message::TogglePopup => {
                if let Some(p) = self.popup.take() {
                    return destroy_popup(p);
                }

                if let Some(main_window_id) = self.core.main_window_id() {
                    let new_id = window::Id::unique();
                    self.popup.replace(new_id);

                    let popup_settings = self.core.applet.get_popup_settings(
                        main_window_id,
                        new_id,
                        None,
                        None,
                        None,
                    );

                    return get_popup(popup_settings);
                }
                warn!("Cannot toggle popup: main window ID not available");
            }
            Message::CloseRequested(id) => {
                if Some(id) == self.popup {
                    self.popup = None;
                }
            }
            Message::DaemonConnected => {
                self.daemon_state = DaemonConnectionState::Connected;
                // Reset retry strategy on successful connection.
                self.retry_strategy.reset();
                // The widget /events subscription is self-healing
                // (`run_widget_subscription` in super-stt-shared owns
                // its own reconnect loop), so we deliberately do NOT
                // bump `udp_restart_counter` here. Restarting the iced
                // subscription on every successful ping would cancel
                // the helper task mid-flight and cause every ping
                // cycle to re-enter `session::obtain` — i.e. another
                // potential keyring touch. Audio settings live in the
                // settings app; the applet doesn't need to fetch them.
            }
            Message::PingResponse {
                message: _,
                connection_active: _,
            } => {
                // A successful `/v1/ping` always means the daemon is
                // reachable — the HTTP protocol carries no separate
                // "connection is marked inactive" path (the legacy
                // protocol used to). Always flip to Connected.
                info!("Daemon ping successful and connection is active - daemon may be idle");
                self.daemon_state = DaemonConnectionState::Connected;
                self.retry_strategy.reset();
            }
            Message::DaemonError(err) => {
                warn!("Daemon error: {err}");

                // Check if we were previously connected (this is a disconnection)
                let was_connected = matches!(self.daemon_state, DaemonConnectionState::Connected);

                if was_connected {
                    // If we were connected and lost connection, reset retry strategy for reconnection
                    self.retry_strategy = RetryStrategy::for_initial_connection();
                    info!("Lost connection to daemon, starting reconnection attempts");
                    return cosmic_app::Task::perform(async {}, |()| {
                        cosmic::Action::App(Message::ScheduleRetry)
                    });
                }
                // Keep trying forever - never give up
                return cosmic_app::Task::perform(async {}, |()| {
                    cosmic::Action::App(Message::ScheduleRetry)
                });
            }
            Message::ScheduleRetry => {
                // Schedule a retry with appropriate delay - retries forever
                self.retry_strategy.should_retry(); // Always returns true, increments counter
                let delay = self.retry_strategy.next_delay();
                info!(
                    "Scheduling daemon connection retry {} in {:?}",
                    self.retry_strategy.attempt, delay
                );

                // Keep showing connecting state with retry information
                self.daemon_state = DaemonConnectionState::Connecting;

                return cosmic_app::Task::perform(
                    async move {
                        tokio::time::sleep(delay).await;
                    },
                    |()| cosmic::Action::App(Message::RetryConnection),
                );
            }
            Message::RecordingStateChanged(state) => {
                // Only allow certain state transitions based on current state
                match (&self.recording_state, &state) {
                    // Allow transition from Processing to Idle (transcription complete)
                    (RecordingState::Processing, RecordingState::Idle) => {
                        info!("Transcription completed: Processing -> Idle");
                        self.recording_state = state;
                    }
                    // Allow any other transition for now (manual state changes, etc.)
                    _ => {
                        self.recording_state = state;
                    }
                }
            }
            Message::RevealerToggle(is_open_src) => {
                self.is_open = if self.is_open == is_open_src {
                    IsOpen::None
                } else {
                    is_open_src
                }
            }
            Message::AudioLevelUpdate { level, is_speech } => {
                self.audio_level = level;
                self.is_speech_detected = is_speech;
                // Update the visualization with new audio data
                self.visualization.update_audio_level(level, is_speech);
            }
            Message::SetVisualizationTheme(theme) => {
                self.theme_config.visualization_theme = theme.clone();
                // Update and save configuration
                self.config
                    .update_visualization_theme(theme.clone(), &self.variant_name);
                // Update visualization theme in-place
                self.visualization.update_theme(theme);
                // Update the visualization with current audio data
                self.visualization
                    .update_audio_level(self.audio_level, self.is_speech_detected);
                self.is_open = IsOpen::None;
            }
            Message::WidgetRecordingState(is_recording) => {
                // Update last-event timestamp (used by the connection
                // health watchdog the same way the UDP path did).
                self.last_udp_data = std::time::Instant::now();

                let new_state = if is_recording {
                    RecordingState::Recording
                } else if matches!(self.recording_state, RecordingState::Recording) {
                    // Just transitioned out of Recording — give the visualizer a
                    // brief Processing state while the daemon transcribes.
                    RecordingState::Processing
                } else {
                    RecordingState::Idle
                };

                let was_recording = matches!(self.recording_state, RecordingState::Recording);
                let will_be_recording = matches!(new_state, RecordingState::Recording);

                self.recording_state = new_state;

                if was_recording && !will_be_recording {
                    self.visualization.clear();
                }
            }
            Message::WidgetFrequencyBands {
                bands,
                sample_rate: _,
                total_energy,
            } => {
                self.last_udp_data = std::time::Instant::now();
                self.visualization
                    .update_frequency_bands(&bands, total_energy);
                self.audio_level = total_energy;
                self.is_speech_detected = total_energy > 0.02;
            }
            #[allow(clippy::cast_precision_loss)]
            Message::WidgetAudioSamples {
                samples,
                sample_rate: _,
                channels: _,
            } => {
                self.last_udp_data = std::time::Instant::now();
                self.visualization.update_audio_samples(&samples);
                let audio_level = if samples.is_empty() {
                    0.0
                } else {
                    let rms: f32 =
                        samples.iter().map(|&s| s * s).sum::<f32>() / samples.len() as f32;
                    rms.sqrt().min(1.0)
                };
                self.audio_level = audio_level;
                self.is_speech_detected = audio_level > 0.02;
            }
            Message::WidgetRevoked(reason) => {
                warn!(
                    "Widget session revoked by daemon (reason={reason}); will re-subscribe via consent flow on next retry"
                );
                // Treat the same way as a connection drop so the
                // existing reconnect path triggers a fresh
                // /auth/request → consent popup → re-subscribe.
                self.daemon_state = DaemonConnectionState::Error(format!("revoked: {reason}"));
            }
            Message::WidgetOtherEvent(_) | Message::WidgetSubscriptionError(_) => {
                // Other events are informational only; subscription
                // errors are logged inside the subscription task.
            }
            Message::WidgetBlocked(reason) => {
                warn!(
                    "Widget subscription blocked by user denial ({reason}); waiting for explicit retry"
                );
                self.daemon_state = DaemonConnectionState::Blocked(reason);
            }
            Message::RetryAuthorization => {
                info!("Retrying authorization after user denial");
                // Drop any cached token (in-memory + keyring) so the
                // next subscription cycle hits the daemon's
                // /auth/request and spawns a fresh consent prompt.
                if let Err(e) = session::forget(APPLET_APP_ID) {
                    warn!("Failed to forget session before retry: {e}");
                }
                // Restart the iced subscription so the dropped
                // helper task re-spawns from scratch.
                self.udp_restart_counter = self.udp_restart_counter.wrapping_add(1);
                self.daemon_state = DaemonConnectionState::Connecting;
                self.retry_strategy = RetryStrategy::for_initial_connection();
            }
            Message::RetryConnection => {
                // Check if this is a manual retry (from error state) or automatic retry
                let is_manual_retry = matches!(self.daemon_state, DaemonConnectionState::Error(_));

                if is_manual_retry {
                    // Reset retry strategy for manual retry
                    self.retry_strategy = RetryStrategy::for_initial_connection();
                    self.daemon_state = DaemonConnectionState::Connecting;
                    info!("Manual retry initiated by user");
                }

                // Try to ping the daemon
                info!(
                    "Retrying daemon connection (attempt {})...",
                    self.retry_strategy.attempt
                );
                return cosmic_app::Task::perform(
                    ping_daemon(self.socket_path.clone()),
                    |result| {
                        cosmic::Action::App(match result {
                            Ok(_) => Message::DaemonConnected,
                            Err(e) => {
                                info!("Retry failed: {e}");
                                Message::ScheduleRetry
                            }
                        })
                    },
                );
            }
            Message::PingTimeout => {
                // Always check daemon health when we think we're connected
                if self.daemon_state == DaemonConnectionState::Connected {
                    // Regularly ping daemon to check if connection is still active
                    return cosmic_app::Task::perform(
                        ping_daemon_with_status(self.socket_path.clone()),
                        |result| {
                            cosmic::Action::App(match result {
                                Ok(response) => Message::PingResponse {
                                    message: response.message,
                                    connection_active: response.connection_active,
                                },
                                Err(e) => {
                                    warn!("Daemon ping failed: {e}");
                                    Message::DaemonError(format!("Connection lost: {e}"))
                                }
                            })
                        },
                    );
                } else if self.daemon_state == DaemonConnectionState::Connecting {
                    // During initial connection, don't interfere with the retry strategy
                    // The retry strategy is already handling the connection attempts
                    // Only log if we've been trying for a while
                    if self.retry_strategy.attempt > 5 {
                        info!(
                            "Still attempting to connect (attempt {})...",
                            self.retry_strategy.attempt
                        );
                    }
                }
                // If in error state, don't spam - wait for manual retry
            }
            Message::OpenGitHub => {
                // Open the GitHub repository in the default browser
                if let Err(e) = std::process::Command::new("xdg-open")
                    .arg(crate::REPOSITORY)
                    .spawn()
                {
                    warn!("Failed to open GitHub URL: {e}");
                }
            }
            Message::LaunchApp => {
                // Launch the Super STT app - try different possible locations
                let launch_attempts = [
                    "super-stt-app",                  // System PATH
                    "./target/debug/super-stt-app",   // Local debug build
                    "./target/release/super-stt-app", // Local release build
                    "/usr/local/bin/super-stt-app",   // Local install
                    "/usr/bin/super-stt-app",         // System install
                ];

                let mut launched = false;

                // First try to find the binary in PATH using 'which'
                if let Ok(output) = std::process::Command::new("which")
                    .arg("super-stt-app")
                    .output()
                    && output.status.success()
                    && let Ok(path) = std::str::from_utf8(&output.stdout)
                {
                    let path = path.trim();
                    if std::process::Command::new(path).spawn().is_ok() {
                        info!("Successfully launched Super STT app from PATH: {path}");
                        launched = true;
                    }
                }

                // If not found in PATH, try other locations
                if !launched {
                    for command in &launch_attempts {
                        if std::process::Command::new(command).spawn().is_ok() {
                            info!("Successfully launched Super STT app with command: {command}");
                            launched = true;
                            break;
                        }
                    }
                }

                if !launched {
                    warn!("Failed to launch Super STT app - tried all common locations");
                }
            }
            Message::SetAppletWidth(width) => {
                self.config.update_applet_width(width, &self.variant_name);
                // Clear and update visualization to ensure it adapts to new size
                self.visualization.clear();
                self.visualization
                    .update_audio_level(self.audio_level, self.is_speech_detected);
                // Don't close settings for slider interactions
            }
            Message::SetShowIcon(show_icon) => {
                self.config.update_show_icon(show_icon, &self.variant_name);
                // Don't close settings for toggle interactions
            }
            Message::SetIconAlignmentEntity(entity) => {
                self.icon_alignment_model.activate(entity);

                let alignment_string = if entity == self.icon_alignment_start {
                    "start".to_string()
                } else if entity == self.icon_alignment_center {
                    "center".to_string()
                } else if entity == self.icon_alignment_end {
                    "end".to_string()
                } else {
                    "start".to_string()
                };

                self.config
                    .update_icon_alignment(alignment_string, &self.variant_name);
                // Don't close settings for alignment changes
            }
            Message::SetShowVisualizations(show_visualizations) => {
                self.config
                    .update_show_visualizations(show_visualizations, &self.variant_name);
                // Don't close settings for toggle interactions
            }

            Message::SetVisualizationColor(color, is_dark) => {
                self.theme_config
                    .visualization_color_config
                    .set_color(color, is_dark);
                let updated_colors = self.theme_config.visualization_color_config.clone();
                self.config
                    .update_visualization_colors(updated_colors.clone(), &self.variant_name);
                // Update colors efficiently without recreating the entire visualization
                self.visualization.update_colors(updated_colors);
                // Don't close settings for color changes
            }

            Message::SetColorThemeEntity(entity) => {
                self.theme_selector_model.activate(entity);

                // Update which theme is selected for color configuration
                if entity == self.theme_selector_light {
                    self.selected_theme_for_config = false; // Light theme
                } else if entity == self.theme_selector_dark {
                    self.selected_theme_for_config = true; // Dark theme
                }
                // No need to save config as this is just UI state
            }
        }
        cosmic_app::Task::none()
    }

    #[allow(clippy::cast_precision_loss, clippy::cast_lossless)]
    fn view(&self) -> Element<'_, Message> {
        // Show visualizations only when daemon is actively recording AND user has visualizations enabled
        let should_show_visualizations = matches!(self.recording_state, RecordingState::Recording)
            && self.config.ui.show_visualization;

        // Get suggested window size from the applet framework
        let (suggested_width, suggested_height) = self.core.applet.suggested_window_size();
        let (_, suggested_padding_h) = self.core.applet.suggested_padding(false);
        let suggested_padding = suggested_padding_h as f32;

        // Calculate appropriate size based on panel orientation and user configuration
        // If visualizations are disabled, use a smaller icon-only size
        let visualization_size = if self.config.ui.show_visualization {
            // When visualizations are enabled, use the configured width
            let configured_width = self.config.ui.applet_width as f32;
            if self.core.applet.is_horizontal() {
                // In horizontal panel, constrain by height but respect user width preference
                #[allow(clippy::cast_precision_loss)]
                let available_height = suggested_height.get() as f32 - (suggested_padding * 2.0);
                let constrained_height = available_height.min(VISUALIZATION_HEIGHT + 8.0);
                // Use configured width directly, only limit by extreme aspect ratios
                let constrained_width = configured_width.min(available_height * 8.0).max(60.0);
                Size::new(constrained_width, constrained_height)
            } else {
                // In vertical panel, use configured width with reasonable limits
                #[allow(clippy::cast_precision_loss)]
                let available_width = suggested_width.get() as f32 - (suggested_padding * 2.0);
                let constrained_width = configured_width.min(available_width * 2.0).max(60.0);
                let constrained_height = VISUALIZATION_HEIGHT + 8.0;
                Size::new(constrained_width, constrained_height)
            }
        } else {
            // When visualizations are disabled, use a compact icon size
            let icon_size = if self.core.applet.is_horizontal() {
                #[allow(clippy::cast_precision_loss)]
                let available_height = suggested_height.get() as f32 - (suggested_padding * 2.0);
                available_height.clamp(24.0, 48.0)
            } else {
                #[allow(clippy::cast_precision_loss)]
                let available_width = suggested_width.get() as f32 - (suggested_padding * 2.0);
                available_width.clamp(24.0, 48.0)
            };
            Size::new(icon_size, icon_size)
        };

        if self.daemon_state == DaemonConnectionState::Connected && should_show_visualizations {
            // Use mouse_area with visualization element

            let visualization_element =
                container(mouse_area(self.visualization.clone()).on_press(Message::TogglePopup))
                    .width(Length::Fixed(visualization_size.width))
                    .height(Length::Fixed(visualization_size.height));

            // Use autosize_window to inform the applet of our desired size
            self.core
                .applet
                .autosize_window(visualization_element)
                .into()
        } else {
            let icon_bytes = if !(self.daemon_state == DaemonConnectionState::Connected
                || self.daemon_state == DaemonConnectionState::Connecting)
            {
                ERROR_ICON
            } else if self.config.ui.show_icon {
                NORMAL_ICON
            } else {
                TRANSPARENT_ICON
            };

            let (applet_padding, _) = self.core.applet.suggested_padding(false);

            let icon_alignment = match self.config.ui.icon_alignment.as_str() {
                "center" => Alignment::Center,
                "end" => Alignment::End,
                _ => Alignment::Start, // Default for "start" and unknown values
            };

            let icon_button = transparent_icon_button(
                icon_bytes,
                visualization_size,
                applet_padding,
                icon_alignment,
            );

            // Reset window size properly when switching back to icon
            self.core.applet.autosize_window(icon_button).into()
        }
    }

    fn view_window(&self, _id: window::Id) -> Element<'_, Message> {
        let content = create_popup_content(&PopupContentParams {
            daemon_state: &self.daemon_state,
            is_open: &self.is_open,
            theme_config: &self.theme_config,
            config: &self.config,
            icon_alignment_model: &self.icon_alignment_model,
            theme_selector_model: &self.theme_selector_model,
            selected_theme_for_config: self.selected_theme_for_config,
        });

        self.core.applet.popup_container(content).into()
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Message> {
        Some(Message::CloseRequested(id))
    }
}

fn transparent_icon_button<'a>(
    icon_bytes: &'static [u8],
    visualization_size: Size,
    applet_padding: u16,
    alignment: Alignment,
) -> cosmic::widget::Button<'a, crate::app::Message> {
    // Calculate appropriate icon size based on panel size, but don't stretch
    let icon_size =
        (visualization_size.height.min(visualization_size.width) * 0.6).clamp(16.0, 32.0);

    button::custom(
        layer_container(
            widget::icon(widget::icon::from_svg_bytes(icon_bytes))
                .class(theme::Svg::Custom(Rc::new(|theme| {
                    iced_widget::svg::Style {
                        color: Some(theme.cosmic().background.on.into()),
                    }
                })))
                .width(Length::Fixed(icon_size))
                .height(Length::Fixed(icon_size)),
        )
        .align_x(alignment)
        .center_y(Length::Fill),
    )
    .width(Length::Fixed(
        visualization_size.width + 2f32 * f32::from(applet_padding),
    ))
    .height(Length::Fixed(
        visualization_size.height + 2f32 * f32::from(applet_padding),
    ))
    .class(Button::AppletIcon)
    .on_press_down(Message::TogglePopup)
}

/// Stable identity used to cache the applet's widget-scope session
/// token under `(super-stt-session, super-stt-cosmic-applet)`. Mirrors
/// the layout the CLI and settings app already use.
const APPLET_APP_ID: AppId = AppId("super-stt-cosmic-applet");
const APPLET_APP_NAME: &str = "Super STT COSMIC Applet";
const APPLET_SCOPE: &str = "widget";
const APPLET_TOPICS: &[&str] = &["recording_state", "frequency_bands", "audio_samples"];

/// Subscribes to the daemon's `GET /events` SSE stream and forwards
/// each event as a typed [`Message`]. The subscription is self-healing
/// — if the SSE stream drops, the daemon revokes the session, or the
/// connection wedges past the keepalive deadline, the shared
/// [`run_widget_subscription`] helper reconnects (with backoff) and
/// re-auths automatically. The wrapping iced subscription only
/// terminates when the applet is shutting down.
fn applet_events_subscription(
    _id: &UdpSubscriptionId,
) -> std::pin::Pin<Box<dyn cosmic::iced::futures::Stream<Item = Message> + Send>> {
    Box::pin(cosmic::iced::stream::channel(100, async |mut channel| {
        let config = WidgetSubscriptionConfig::new(
            APPLET_APP_ID,
            APPLET_APP_NAME,
            APPLET_SCOPE,
            APPLET_TOPICS,
        );
        let mut updates = Box::pin(run_widget_subscription(get_http_socket_path(), config));
        info!("Widget subscription starting");
        while let Some(update) = updates.next().await {
            let msg = applet_subscription_update_to_message(update);
            if channel.send(msg).await.is_err() {
                break; // applet shutting down
            }
        }
        info!("Widget subscription ended");
    }))
}

/// Project a [`WidgetSubscriptionUpdate`] from the shared helper into
/// the applet's typed [`Message`] enum.
fn applet_subscription_update_to_message(update: WidgetSubscriptionUpdate) -> Message {
    match update {
        // Route a successful (re)connect into the existing
        // `DaemonConnected` handler so it clears any prior
        // `Error("revoked: …")` state and resets retry. Without this,
        // a user-denied → daemon-restart → auto-reconnect cycle would
        // leave the UI stuck on the stale "revoked" error even though
        // the subscription is live again.
        WidgetSubscriptionUpdate::Connected => Message::DaemonConnected,
        WidgetSubscriptionUpdate::Event(evt) => widget_event_to_message(evt),
        WidgetSubscriptionUpdate::Disconnected { reason } => {
            warn!("Widget /events disconnected ({reason}); reconnecting");
            Message::WidgetSubscriptionError(reason)
        }
        WidgetSubscriptionUpdate::NeedsReauth { reason } => {
            warn!("Widget session needs re-auth ({reason}); will re-consent on next attempt");
            Message::WidgetRevoked(reason)
        }
        WidgetSubscriptionUpdate::Blocked { reason } => {
            warn!("Widget subscription blocked ({reason}); stream terminated");
            Message::WidgetBlocked(reason)
        }
    }
}

/// Translate one [`super_stt_shared::daemon::http_client::WidgetEvent`]
/// into the applet's typed [`Message`] enum. Unknown event names are
/// surfaced as `WidgetOtherEvent(name)` so the update loop can log
/// them at most once per event type.
fn widget_event_to_message(evt: super_stt_shared::daemon::http_client::WidgetEvent) -> Message {
    use serde_json::Value;
    fn b64_to_f32_vec(s: Option<&str>) -> Vec<f32> {
        let Some(s) = s else { return Vec::new() };
        let Ok(bytes) = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, s)
        else {
            return Vec::new();
        };
        bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()
    }

    let p: &Value = &evt.payload;
    match evt.name.as_str() {
        "recording_state" => Message::WidgetRecordingState(
            p.get("is_recording")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        ),
        "frequency_bands" => Message::WidgetFrequencyBands {
            bands: b64_to_f32_vec(p.get("bands_b64").and_then(Value::as_str)),
            #[allow(clippy::cast_possible_truncation)]
            sample_rate: p.get("sample_rate").and_then(Value::as_f64).unwrap_or(0.0) as f32,
            #[allow(clippy::cast_possible_truncation)]
            total_energy: p.get("total_energy").and_then(Value::as_f64).unwrap_or(0.0) as f32,
        },
        "audio_samples" => Message::WidgetAudioSamples {
            samples: b64_to_f32_vec(p.get("samples_b64").and_then(Value::as_str)),
            #[allow(clippy::cast_possible_truncation)]
            sample_rate: p.get("sample_rate").and_then(Value::as_f64).unwrap_or(0.0) as f32,
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            channels: p.get("channels").and_then(Value::as_u64).unwrap_or(1) as u16,
        },
        "revoked" => Message::WidgetRevoked(
            p.get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
        ),
        "subscribed" | "error" => Message::WidgetOtherEvent(evt.name),
        other => Message::WidgetOtherEvent(other.to_string()),
    }
}

#[cfg(test)]
mod widget_subscription_mapping_tests {
    //! These tests pin the mapping between the shared
    //! `WidgetSubscriptionUpdate` variants and the applet's `Message`
    //! enum. Each variant has a load-bearing UX contract:
    //!
    //! - `Connected` → `Message::DaemonConnected` so the existing
    //!   handler clears any prior `Error("revoked: …")` state after
    //!   auto-recovery. If this mapping silently changes, the applet
    //!   gets stuck on a stale revoked banner forever.
    //! - `Blocked` → `Message::WidgetBlocked` so the UI flips to the
    //!   sticky "Authorization denied" view with a Retry button. If
    //!   this maps to anything else, the helper's
    //!   stop-spamming-on-deny fix becomes invisible.
    //! - `NeedsReauth` → `Message::WidgetRevoked` so the UI shows a
    //!   transient revoked banner while the helper does the
    //!   `session::forget` → fresh-consent cycle.
    //! - `Disconnected` → `Message::WidgetSubscriptionError` so the
    //!   UI doesn't change state during the helper's internal
    //!   backoff/reconnect — the helper auto-recovers.
    use super::*;
    use super_stt_shared::daemon::http_client::WidgetEvent;

    #[test]
    fn blocked_maps_to_widget_blocked_with_reason() {
        let update = WidgetSubscriptionUpdate::Blocked {
            reason: "auth_denied (user_denied_cached)".to_string(),
        };
        match applet_subscription_update_to_message(update) {
            Message::WidgetBlocked(reason) => {
                assert_eq!(reason, "auth_denied (user_denied_cached)");
            }
            other => panic!("Blocked must map to Message::WidgetBlocked, got {other:?}"),
        }
    }

    #[test]
    fn needs_reauth_maps_to_widget_revoked_with_reason() {
        let update = WidgetSubscriptionUpdate::NeedsReauth {
            reason: "invalid_session (expired)".to_string(),
        };
        match applet_subscription_update_to_message(update) {
            Message::WidgetRevoked(reason) => {
                assert_eq!(reason, "invalid_session (expired)");
            }
            other => panic!("NeedsReauth must map to Message::WidgetRevoked, got {other:?}"),
        }
    }

    #[test]
    fn connected_maps_to_daemon_connected_for_state_clear() {
        // Critical regression guard: an earlier bug had this mapping
        // to a no-op `WidgetOtherEvent`, which left a stale
        // `Error("revoked: …")` banner up after auto-recovery.
        let update = WidgetSubscriptionUpdate::Connected;
        assert!(matches!(
            applet_subscription_update_to_message(update),
            Message::DaemonConnected
        ));
    }

    #[test]
    fn disconnected_maps_to_subscription_error() {
        let update = WidgetSubscriptionUpdate::Disconnected {
            reason: "stream ended".to_string(),
        };
        match applet_subscription_update_to_message(update) {
            Message::WidgetSubscriptionError(reason) => {
                assert_eq!(reason, "stream ended");
            }
            other => {
                panic!("Disconnected must map to Message::WidgetSubscriptionError, got {other:?}")
            }
        }
    }

    #[test]
    fn revoked_widget_event_maps_to_widget_revoked() {
        let evt = WidgetEvent {
            name: "revoked".to_string(),
            payload: serde_json::json!({ "reason": "exe_changed" }),
        };
        match widget_event_to_message(evt) {
            Message::WidgetRevoked(reason) => assert_eq!(reason, "exe_changed"),
            other => panic!("revoked event must map to WidgetRevoked, got {other:?}"),
        }
    }
}
