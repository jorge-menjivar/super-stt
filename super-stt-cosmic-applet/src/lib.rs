// SPDX-License-Identifier: GPL-3.0-only
mod app;
mod config;
mod daemon;
mod models;
mod ui;

use cosmic::{
    app as cosmic_app,
    iced::{
        platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup},
        window, Alignment, Subscription,
    },
    iced_widget,
    theme::{self, Button},
    widget::{
        self, button, container, layer_container, mouse_area,
        segmented_button::{Entity, SingleSelectModel},
    },
    Element,
};

use futures_util::SinkExt;
use log::{info, warn};
use std::{path::PathBuf, rc::Rc};
use tokio::net::UdpSocket;

// Cache icon bytes to avoid allocation on every render
static NORMAL_ICON: &[u8] = include_bytes!("../resources/assets/super-stt-icon.svg");
static TRANSPARENT_ICON: &[u8] = include_bytes!("../resources/assets/transparent-icon.svg");
static ERROR_ICON: &[u8] = include_bytes!("../resources/assets/error-icon.svg");

use crate::models::state::{DaemonConnectionState, RecordingState};
use crate::ui::components::sound_visualization::VisualizationComponent;
use crate::{app::Message, models::state::IsOpen};
use crate::{
    config::AppletConfig,
    ui::views::{create_popup_content, PopupContentParams},
};
use crate::{
    daemon::{
        client::load_audio_themes, fetch_daemon_config, ping_daemon, ping_daemon_with_status,
        set_and_test_audio_theme, RetryStrategy, TokenBucketRateLimiter,
    },
    models::theme::ThemeConfig,
};
use super_stt_shared::{
    parse_audio_samples_from_udp, parse_frequency_bands_from_udp, parse_recording_state_from_udp,
    theme::AudioTheme, UdpAuth,
};

// Connection monitoring constants
const PING_INTERVAL_SECS: u64 = 5; // Ping every 5 seconds to check daemon health
const VISUALIZATION_HEIGHT: f32 = 100.0; // Visualization height in pixels

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
    available_audio_themes: Vec<AudioTheme>,
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
            audio_theme: config.audio.theme,
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
            socket_path: super_stt_shared::validation::get_secure_socket_path(),
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
            available_audio_themes: Vec::new(), // Will be loaded when daemon connects
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

    fn style(&self) -> Option<cosmic::iced_runtime::Appearance> {
        Some(cosmic::applet::style())
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            // UDP subscription for audio level monitoring that restarts when daemon reconnects
            Subscription::run_with_id(
                self.udp_restart_counter,
                cosmic::iced::stream::channel(100, |mut channel| async move {
                    let socket = match UdpSocket::bind("127.0.0.1:0").await {
                        Ok(socket) => socket,
                        Err(e) => {
                            warn!("Failed to bind UDP socket: {e}");
                            futures_util::future::pending().await
                        }
                    };

                    // Register with daemon using authentication
                    let auth = match UdpAuth::new() {
                        Ok(auth) => auth,
                        Err(e) => {
                            warn!("Failed to initialize UDP authentication: {e}");
                            return;
                        }
                    };

                    let registration_msg = match auth.create_auth_message("applet") {
                        Ok(msg) => msg,
                        Err(e) => {
                            warn!("Failed to create authenticated registration message: {e}");
                            return;
                        }
                    };

                    if let Err(e) = socket
                        .send_to(registration_msg.as_bytes(), "127.0.0.1:8765")
                        .await
                    {
                        warn!("Failed to register with daemon: {e}");
                        return;
                    }

                    // Test if registration was successful by sending a test message
                    if let Err(e) = socket.send_to(b"PING", "127.0.0.1:8765").await {
                        warn!("Failed to send ping to daemon: {e}");
                    }

                    let mut buffer = [0u8; 1024];
                    let mut rate_limiter = TokenBucketRateLimiter::for_audio_processing();
                    let mut keepalive_interval =
                        tokio::time::interval(tokio::time::Duration::from_secs(60));

                    loop {
                        tokio::select! {
                            // Handle incoming UDP data
                            recv_result = socket.recv_from(&mut buffer) => {
                                match recv_result {
                                    Ok((len, _addr)) => {
                                        // Apply rate limiting to prevent UDP flooding DoS attacks
                                        if !rate_limiter.try_consume() {
                                            // Rate limited - drop packet and log warning
                                            warn!("UDP packet rate limit exceeded, dropping packet");

                                            // Optional: Add a small delay to further throttle rapid senders
                                            if let Some(delay) = rate_limiter.time_until_next_token() {
                                                tokio::time::sleep(
                                                    delay.min(std::time::Duration::from_millis(10)),
                                                )
                                                .await;
                                            }
                                            continue;
                                        }

                                        let data = buffer[..len].to_vec();
                                        if channel.send(Message::UdpData(data)).await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        warn!("UDP receive error: {e}");
                                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                                    }
                                }
                            }
                            // Send periodic keep-alive pings
                            _ = keepalive_interval.tick() => {
                                // Send keep-alive ping to maintain connection
                                if let Err(e) = socket.send_to(b"PING", "127.0.0.1:8765").await {
                                    warn!("Failed to send UDP keep-alive: {e}");
                                }
                            }
                        }
                    }
                }),
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
                // Reset retry strategy on successful connection
                self.retry_strategy.reset();
                // Restart UDP subscription when daemon reconnects
                self.udp_restart_counter += 1;
                info!(
                    "Daemon connected, restarting UDP subscription (counter: {})",
                    self.udp_restart_counter
                );

                // Fetch daemon configuration and load available themes in parallel
                let socket_path = self.socket_path.clone();
                let socket_path_themes = self.socket_path.clone();

                return cosmic_app::Task::batch([
                    cosmic_app::Task::perform(fetch_daemon_config(socket_path), |result| {
                        match result {
                            Ok(config) => {
                                cosmic::Action::App(Message::DaemonConfigReceived(config))
                            }
                            Err(err) => {
                                warn!("Failed to fetch daemon config: {err}");
                                cosmic::Action::App(Message::DaemonError(format!(
                                    "Failed to fetch config: {err}"
                                )))
                            }
                        }
                    }),
                    cosmic_app::Task::perform(load_audio_themes(socket_path_themes), |themes| {
                        cosmic::Action::App(Message::AudioThemesLoaded(themes))
                    }),
                ]);
            }
            Message::PingResponse {
                message: _,
                connection_active,
            } => {
                if connection_active {
                    info!("Daemon ping successful and connection is active - daemon may be idle");
                    // Connection is still active, no need to reconnect
                    self.daemon_state = DaemonConnectionState::Connected;
                    self.retry_strategy.reset();
                } else {
                    warn!(
                        "Daemon responded but connection is marked as inactive - forcing reconnect"
                    );
                    // Connection is broken, need to reconnect
                    self.daemon_state =
                        DaemonConnectionState::Error("Connection inactive".to_string());
                    self.retry_strategy = RetryStrategy::for_initial_connection();
                    // Trigger reconnection
                    return cosmic_app::Task::perform(async {}, |()| {
                        cosmic::Action::App(Message::RetryConnection)
                    });
                }
            }
            Message::DaemonConfigReceived(config) => {
                // Parse daemon configuration and sync theme settings
                if let Some(audio_config) = config
                    .get("audio")
                    .and_then(|audio| audio.get("theme"))
                    .and_then(|theme| theme.as_str())
                {
                    let daemon_audio_theme = audio_config.parse::<AudioTheme>().unwrap_or_default();
                    info!("Received daemon config, syncing audio theme: {daemon_audio_theme:?}");

                    // Update local theme config with daemon's audio theme
                    self.theme_config.audio_theme = daemon_audio_theme;

                    // Save the updated theme config
                    self.config
                        .update_audio_theme(daemon_audio_theme, &self.variant_name);
                } else {
                    warn!("No audio theme found in daemon configuration");
                }
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
            Message::SetAudioTheme(theme) => {
                self.theme_config.audio_theme = theme;
                // Update and save configuration
                self.config.update_audio_theme(theme, &self.variant_name);
                return cosmic_app::Task::perform(
                    set_and_test_audio_theme(self.socket_path.clone(), theme.to_string()),
                    |result| {
                        cosmic::Action::App(match result {
                            Ok(_) => Message::DaemonConnected,
                            Err(e) => Message::DaemonError(e),
                        })
                    },
                );
            }
            Message::AudioThemesLoaded(themes) => {
                self.available_audio_themes = themes;
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
            #[allow(clippy::cast_precision_loss)]
            Message::UdpData(data) => {
                // Update last UDP data timestamp
                self.last_udp_data = std::time::Instant::now();

                // Try to parse as recording state first
                if let Ok(state_data) = parse_recording_state_from_udp(&data) {
                    let new_state = if state_data.is_recording {
                        RecordingState::Recording
                    } else {
                        // Recording stopped - transition to Processing to show transcription is happening
                        // Only transition to Processing if we were Recording before
                        if matches!(self.recording_state, RecordingState::Recording) {
                            RecordingState::Processing
                        } else {
                            RecordingState::Idle
                        }
                    };

                    // Clear visualization data when transitioning away from recording
                    let was_recording = matches!(self.recording_state, RecordingState::Recording);
                    let will_be_recording = matches!(new_state, RecordingState::Recording);

                    self.recording_state = new_state;

                    // Clear the visualization when stopping recording to prevent artifacts
                    if was_recording && !will_be_recording {
                        self.visualization.clear();
                    }
                } else if let Ok(frequency_data) = parse_frequency_bands_from_udp(&data) {
                    // Update visualization with pre-computed frequency bands
                    self.visualization
                        .update_frequency_bands(&frequency_data.bands, frequency_data.total_energy);

                    // Use total energy for audio level and speech detection
                    self.audio_level = frequency_data.total_energy;
                    self.is_speech_detected = frequency_data.total_energy > 0.02;
                } else if let Ok(samples_data) = parse_audio_samples_from_udp(&data) {
                    // Update visualization with real audio samples for frequency analysis
                    self.visualization
                        .update_audio_samples(&samples_data.samples);

                    // Calculate overall audio level from samples for state management
                    let audio_level = if samples_data.samples.is_empty() {
                        0.0
                    } else {
                        let rms: f32 = samples_data.samples.iter().map(|&s| s * s).sum::<f32>()
                            / samples_data.samples.len() as f32;
                        rms.sqrt().min(1.0)
                    };

                    self.audio_level = audio_level;
                    // Speech detection based on audio activity
                    self.is_speech_detected = audio_level > 0.02;
                }
                // Note: Unknown packets (like registration acks) are silently ignored
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
                {
                    if output.status.success() {
                        if let Ok(path) = std::str::from_utf8(&output.stdout) {
                            let path = path.trim();
                            if std::process::Command::new(path).spawn().is_ok() {
                                info!("Successfully launched Super STT app from PATH: {path}");
                                launched = true;
                            }
                        }
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
            available_audio_themes: &self.available_audio_themes,
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
