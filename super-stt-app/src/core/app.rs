// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::client::{
    RecordEvent, cancel_download, get_allow_online_models, get_current_audio_theme,
    get_current_device, get_current_model, get_custom_models_dir, get_download_status,
    get_preview_typing, get_recording_stop_mode, get_volume, get_write_method,
    list_available_models, load_audio_themes, ping_daemon, record_command_stream,
    set_allow_online_models, set_and_test_audio_theme, set_audio_theme, set_custom_models_dir,
    set_device, set_model, set_preview_typing, set_recording_stop_mode, set_volume,
    set_write_method, stop_record_command, test_daemon_connection,
};
use crate::state::{AudioTheme, ContextPage, DaemonStatus, MenuAction, Page, RecordingStatus};
use crate::ui::messages::Message;
use crate::ui::views;
use cosmic::app::context_drawer;
use cosmic::iced::Subscription;
use cosmic::prelude::*;
use cosmic::widget::{icon, menu, nav_bar};
use futures_util::{SinkExt, StreamExt};
use log::{info, warn};
use std::collections::HashMap;
use std::path::PathBuf;
use super_stt_shared::models::provider::{OnlineProvider, Provider};
use super_stt_shared::models::registry::{self, SourceKind};
use tokio::time::Duration;

/// Unified model operation state that encompasses downloading, loading, and switching
#[derive(Debug, Clone)]
pub enum ModelOperationState {
    /// Model is ready for use
    Ready,
    /// Downloading model files with progress information
    Downloading {
        target_model: String,
        progress: super_stt_shared::models::protocol::DownloadProgress,
    },
    /// Loading model into memory (after download completed)
    Loading {
        target_model: String,
        status_message: String,
    },
    /// Model operation failed
    Error { message: String },
}

/// Device switching state
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceState {
    Ready,
    Switching {
        target_device: String,
        status_message: String,
    },
    Cooldown, // Brief period after switching to avoid premature device requests
}

/// The application model stores app-specific state used to describe its interface and
/// drive its logic.
#[allow(clippy::struct_excessive_bools)]
pub struct AppModel {
    /// Application state which is managed by the COSMIC runtime.
    core: cosmic::Core,
    /// Display a context drawer with the designated page if defined.
    context_page: ContextPage,
    /// Contains items assigned to the nav bar panel.
    nav: nav_bar::Model,

    // Super STT specific state
    /// Socket path for daemon communication
    pub socket_path: PathBuf,
    /// Current daemon connection status
    pub daemon_status: DaemonStatus,
    /// Current recording status
    pub recording_status: RecordingStatus,
    /// Latest transcription text
    pub transcription_text: String,
    /// Current audio level (0.0 to 1.0)
    pub audio_level: f32,
    /// Whether speech is currently detected
    pub is_speech_detected: bool,
    /// Available audio themes
    pub audio_themes: Vec<AudioTheme>,
    /// Currently selected audio theme
    pub selected_audio_theme: AudioTheme,
    /// Last non-silent theme (to restore when toggling audio feedback back on)
    pub last_non_silent_theme: AudioTheme,
    /// UDP restart counter for subscriptions
    pub udp_restart_counter: u64,
    /// Last UDP data timestamp
    pub last_udp_data: std::time::Instant,

    // Model management state
    /// Search input for model selection context drawer
    pub model_search: String,
    /// Available models from daemon as `(name, provider)` pairs.
    pub available_models: Vec<(String, Provider, SourceKind)>,
    /// Currently loaded model
    pub current_model: String,
    /// Provider of the currently loaded model
    pub current_provider: Provider,
    /// Source kind of the currently loaded model
    pub current_source: SourceKind,
    /// The model we had before starting a download (to revert to on cancel)
    pub previous_model: String,
    /// Provider of the previous model
    pub previous_provider: Provider,
    /// Source kind of the previous model
    pub previous_source: SourceKind,
    /// Model operation state (downloading, loading, or ready)
    pub model_operation_state: ModelOperationState,

    // Device management state
    /// Current device (cpu/cuda) from daemon
    pub current_device: String,
    /// Available devices from daemon
    pub available_devices: Vec<String>,
    /// GPU memory info: (free, total) in bytes. None if CUDA unavailable.
    pub gpu_memory: super_stt_shared::daemon::client::GpuMemoryInfo,
    /// Device switching state
    pub device_state: DeviceState,
    /// Timestamp of last device switch to avoid polling too soon
    pub last_device_switch: Option<std::time::Instant>,
    /// Last event timestamp for polling daemon events
    pub last_event_timestamp: Option<String>,

    // Preview typing state
    /// Whether preview typing is enabled (beta feature)
    pub preview_typing_enabled: bool,

    // Recording stop mode
    pub recording_stop_mode: super_stt_shared::models::recording_stop_mode::RecordingStopMode,

    // Write method
    pub write_method: super_stt_shared::models::write_method::WriteMethod,

    // Master volume (0-100)
    pub volume: u8,

    // Custom models directory
    pub custom_models_dir: Option<String>,
    pub custom_models_dir_input: String,
    pub custom_models_dir_editing: bool,

    // Online models state
    pub allow_online_models: bool,
    pub openai_api_key_input: String,
    pub has_openai_api_key: bool,
    pub mistral_api_key_input: String,
    pub has_mistral_api_key: bool,
    pub deepgram_api_key_input: String,
    pub has_deepgram_api_key: bool,
}

/// Create a COSMIC application from the app model
impl cosmic::Application for AppModel {
    /// The async executor that will be used to run your application's commands.
    type Executor = cosmic::executor::Default;

    /// Data that your application receives to its init method.
    type Flags = ();

    /// Messages which the application and its widgets will emit.
    type Message = Message;

    /// Unique identifier in RDNN (reverse domain name notation) format.
    const APP_ID: &'static str = "ai.menjivar.super-stt-app";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    /// Initializes the application with any given flags and startup commands.
    fn init(
        core: cosmic::Core,
        _flags: Self::Flags,
    ) -> (Self, Task<cosmic::Action<Self::Message>>) {
        // Create a nav bar with Super STT specific pages
        let mut nav = nav_bar::Model::default();

        nav.insert()
            .text("Customization")
            .data::<Page>(Page::Customization)
            .icon(icon::from_name("preferences-desktop-symbolic"))
            .activate();

        nav.insert()
            .text("Recording")
            .data::<Page>(Page::Recording)
            .icon(icon::from_name("audio-input-microphone-symbolic"));

        nav.insert()
            .text("Input Simulation")
            .data::<Page>(Page::InputSimulation)
            .icon(icon::from_name("input-keyboard-symbolic"));

        nav.insert()
            .text("Models")
            .data::<Page>(Page::Models)
            .icon(icon::from_name("applications-science-symbolic"));

        nav.insert()
            .text("Online Models")
            .data::<Page>(Page::OnlineModels)
            .icon(icon::from_name("network-wireless-symbolic"));

        nav.insert()
            .text("Connection")
            .data::<Page>(Page::Connection)
            .icon(icon::from_name("help-about-symbolic"));

        // Construct the app model with the runtime's core.
        let mut app = AppModel {
            core,
            context_page: ContextPage::default(),
            nav,
            // Initialize Super STT state using proper socket path
            socket_path: super_stt_shared::validation::get_secure_socket_path(),
            daemon_status: DaemonStatus::Disconnected,
            recording_status: RecordingStatus::Idle,
            transcription_text: String::new(),
            audio_level: 0.0,
            is_speech_detected: false,
            audio_themes: Vec::new(),
            selected_audio_theme: AudioTheme::default(),
            last_non_silent_theme: AudioTheme::default(),
            udp_restart_counter: 0,
            last_udp_data: std::time::Instant::now(),

            // Initialize model state
            model_search: String::new(),
            available_models: Vec::new(),
            current_model: registry::default_definition().name.to_string(),
            current_provider: registry::default_definition().provider,
            current_source: registry::default_definition().source.kind(),
            previous_model: registry::default_definition().name.to_string(),
            previous_provider: registry::default_definition().provider,
            previous_source: registry::default_definition().source.kind(),
            model_operation_state: ModelOperationState::Loading {
                target_model: registry::default_definition().name.to_string(),
                status_message: "Loading initial model state...".to_string(),
            },

            // Initialize device state
            current_device: String::new(), // Empty until loaded from daemon
            available_devices: vec!["cpu".to_string()], // Default until loaded from daemon
            gpu_memory: None,
            device_state: DeviceState::Ready,
            last_device_switch: None,
            last_event_timestamp: None,

            // Initialize preview typing state (disabled by default as beta feature)
            preview_typing_enabled: false,
            recording_stop_mode:
                super_stt_shared::models::recording_stop_mode::RecordingStopMode::default(),
            write_method: super_stt_shared::models::write_method::WriteMethod::default(),
            volume: 100,

            // Custom models directory
            custom_models_dir: None,
            custom_models_dir_input: String::new(),
            custom_models_dir_editing: false,

            // Online models state
            allow_online_models: false,
            openai_api_key_input: String::new(),
            has_openai_api_key: false,
            mistral_api_key_input: String::new(),
            has_mistral_api_key: false,
            deepgram_api_key_input: String::new(),
            has_deepgram_api_key: false,
        };

        // Create startup commands
        let title_command = app.update_title();

        // Load audio themes on startup (always available)
        let load_themes = Task::perform(load_audio_themes(), |themes| {
            cosmic::Action::App(Message::AudioThemesLoaded(themes))
        });

        // Try to ping the daemon on startup
        let initial_ping = Task::perform(ping_daemon(), |result| {
            cosmic::Action::App(match result {
                Ok(_) => Message::DaemonConnected,
                Err(e) => Message::DaemonError(e),
            })
        });

        // Load initial data (models + device info) on startup
        let load_initial_data = Task::perform(
            async move {
                // Small delay to let daemon connection establish
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            },
            |()| cosmic::Action::App(Message::LoadInitialData),
        );

        (
            app,
            Task::batch([title_command, load_themes, initial_ping, load_initial_data]),
        )
    }

    /// Elements to pack at the start of the header bar.
    fn header_start(&self) -> Vec<Element<'_, Self::Message>> {
        let menu_bar = menu::bar(vec![menu::Tree::with_children(
            menu::root("View").apply(Element::from),
            menu::items(
                &HashMap::new(),
                vec![menu::Item::Button("About", None, MenuAction::About)],
            ),
        )]);

        vec![menu_bar.into()]
    }

    /// Enables the COSMIC application to create a nav bar with this model.
    fn nav_model(&self) -> Option<&nav_bar::Model> {
        // Only show navigation when daemon is connected
        if self.daemon_status == DaemonStatus::Connected {
            Some(&self.nav)
        } else {
            None
        }
    }

    /// Display a context drawer if the context page is requested.
    fn context_drawer(&self) -> Option<context_drawer::ContextDrawer<'_, Self::Message>> {
        if !self.core.window.show_context {
            return None;
        }

        Some(match self.context_page {
            ContextPage::About => context_drawer::context_drawer(
                views::about::page(),
                Message::ToggleContextPage(ContextPage::About),
            )
            .title("About"),

            ContextPage::ModelSelection => {
                let device_switching = matches!(
                    self.device_state,
                    DeviceState::Switching { .. } | DeviceState::Cooldown
                );
                let gpu_enabled = self.current_device == "cuda";
                let has_openai = self.has_openai_api_key;
                let has_mistral = self.has_mistral_api_key;
                let has_deepgram = self.has_deepgram_api_key;

                let filtered_models: Vec<(String, Provider, SourceKind)> = self
                    .available_models
                    .iter()
                    .filter(|(_, provider, source)| {
                        // Customs are always shown
                        if matches!(source, SourceKind::Custom) {
                            return true;
                        }
                        match provider {
                            Provider::LocalWhisper => true,
                            Provider::LocalVoxtral => gpu_enabled,
                            Provider::Online(OnlineProvider::OpenAI) => {
                                self.allow_online_models && has_openai
                            }
                            Provider::Online(OnlineProvider::Mistral) => {
                                self.allow_online_models && has_mistral
                            }
                            Provider::Online(OnlineProvider::Deepgram) => {
                                self.allow_online_models && has_deepgram
                            }
                        }
                    })
                    .cloned()
                    .collect();
                context_drawer::context_drawer(
                    views::models::model_selection_list(
                        &filtered_models,
                        &self.current_model,
                        self.current_provider,
                        self.current_source,
                        &self.model_search,
                        gpu_enabled,
                        self.gpu_memory,
                    ),
                    Message::ToggleContextPage(ContextPage::ModelSelection),
                )
                .title("Select Model")
                .header(views::models::model_drawer_header(
                    &self.model_search,
                    &self.current_device,
                    &self.available_devices,
                    device_switching,
                    self.gpu_memory,
                ))
            }
        })
    }

    /// Describes the interface based on the current state of the application model.
    ///
    /// Application events will be processed through the view. Any messages emitted by
    /// events received by widgets will be passed to the update method.
    fn view(&self) -> Element<'_, Self::Message> {
        // Force Connection page when daemon is not connected
        if self.daemon_status != DaemonStatus::Connected {
            return views::connection::page(
                &self.daemon_status,
                self.socket_path.to_string_lossy().to_string(),
            );
        }

        // When connected, show normal navigation
        let active_page = self
            .nav
            .data::<Page>(self.nav.active())
            .unwrap_or(&Page::Customization);

        match active_page {
            Page::Customization => views::customization::page(
                &self.audio_themes,
                &self.selected_audio_theme,
                self.volume,
            ),
            Page::Recording => views::recording::page(
                self.recording_stop_mode,
                self.preview_typing_enabled,
                &self.recording_status,
                &self.transcription_text,
                self.audio_level,
                self.is_speech_detected,
            ),
            Page::InputSimulation => views::input_simulation::page(self.write_method),
            Page::Models => views::models::page(
                &self.current_model,
                &self.model_operation_state,
                &self.device_state,
                self.custom_models_dir.as_deref(),
                &self.custom_models_dir_input,
                self.custom_models_dir_editing,
            ),
            Page::OnlineModels => views::online_models::page(
                self.allow_online_models,
                self.has_openai_api_key,
                &self.openai_api_key_input,
                self.has_mistral_api_key,
                &self.mistral_api_key_input,
                self.has_deepgram_api_key,
                &self.deepgram_api_key_input,
            ),
            Page::Connection => views::connection::page(
                &self.daemon_status,
                self.socket_path.to_string_lossy().to_string(),
            ),
        }
    }

    /// Register subscriptions for this application.
    ///
    /// Subscriptions are long-running async tasks running in the background which
    /// emit messages to the application through a channel. They are started at the
    /// beginning of the application, and persist through its lifetime.
    fn subscription(&self) -> Subscription<Self::Message> {
        // Connection monitoring constants
        const PING_INTERVAL_SECS: u64 = 5;

        Subscription::batch(vec![
            // UDP audio level streaming subscription with restart capability
            Subscription::run_with(
                UdpSubscriptionId(self.udp_restart_counter),
                audio_events_subscription,
            ),
            // Periodic connection monitoring
            cosmic::iced::time::every(std::time::Duration::from_secs(PING_INTERVAL_SECS))
                .map(|_| Message::PingTimeout),
            // Event subscription for daemon events (stable subscription that handles reconnection internally)
            Subscription::run_with(
                DaemonSocketPath(self.socket_path.clone()),
                daemon_event_subscription,
            ),
        ])
    }

    /// Handles messages emitted by the application and its widgets.
    ///
    /// Tasks may be returned for asynchronous execution of code in the background
    /// on the application's async runtime.
    #[allow(clippy::too_many_lines)]
    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        // Try daemon-related messages first
        if matches!(
            message,
            Message::ConnectToDaemon
                | Message::DaemonConnectionResult(_)
                | Message::DaemonConnected
                | Message::CurrentAudioThemeLoaded(_)
                | Message::VolumeLoaded(_)
                | Message::CustomModelsDirLoaded(_)
                | Message::DaemonError(_)
                | Message::RetryConnection
                | Message::WidgetBlocked(_)
                | Message::RetryAuthorization
                | Message::RefreshDaemonStatus
                | Message::PingTimeout
                | Message::DaemonEventsReceived(_)
                | Message::DaemonEventsError(_)
        ) {
            return self.handle_daemon_messages(message);
        }

        // Model search in context drawer
        if let Message::ModelSearchChanged(ref search) = message {
            self.model_search = search.clone();
            return Task::none();
        }

        // Try model-related messages
        if matches!(
            message,
            Message::LoadInitialData
                | Message::ModelSelected { .. }
                | Message::ModelsLoaded { .. }
                | Message::AvailableModelsLoaded(_)
                | Message::CurrentModelLoaded { .. }
                | Message::ModelChanged { .. }
                | Message::ModelError(_)
        ) {
            return self.handle_model_messages(message);
        }

        // Try device-related messages
        if matches!(
            message,
            Message::DeviceSelected(_)
                | Message::DeviceLoaded(_)
                | Message::DeviceInfoLoaded(_, _, _)
                | Message::DeviceError(_)
        ) {
            return self.handle_device_messages(message);
        }

        // Try download-related messages
        if matches!(
            message,
            Message::DownloadProgressUpdate(_)
                | Message::CancelDownload
                | Message::DownloadCompleted(_)
                | Message::DownloadCancelled(_)
                | Message::DownloadError { .. }
                | Message::CheckDownloadStatus
                | Message::NoDownloadInProgress
        ) {
            return self.handle_download_messages(message);
        }

        // Try preview typing-related messages
        if matches!(
            message,
            Message::PreviewTypingToggled(_)
                | Message::PreviewTypingSettingLoaded(_)
                | Message::PreviewTypingError(_)
        ) {
            return self.handle_preview_typing_messages(message);
        }

        if matches!(
            message,
            Message::RecordingStopModeChanged(_)
                | Message::RecordingStopModeLoaded(_)
                | Message::RecordingStopModeError(_)
        ) {
            return self.handle_recording_stop_mode_messages(message);
        }

        if matches!(
            message,
            Message::WriteMethodChanged(_)
                | Message::WriteMethodLoaded(_)
                | Message::WriteMethodError(_)
        ) {
            return self.handle_write_method_messages(message);
        }

        match &message {
            Message::CustomModelsDirInput(input) => {
                self.custom_models_dir_input = input.clone();
                return Task::none();
            }
            Message::CustomModelsDirEdit(editing) => {
                self.custom_models_dir_editing = *editing;
                if *editing {
                    self.custom_models_dir_input =
                        self.custom_models_dir.clone().unwrap_or_default();
                }
                return Task::none();
            }
            Message::CustomModelsDirSet(path) => {
                let path = path.clone();
                self.custom_models_dir_input = path.as_deref().unwrap_or_default().to_string();
                self.custom_models_dir_editing = false;
                self.custom_models_dir.clone_from(&path);
                return Task::perform(
                    async move {
                        set_custom_models_dir(path).await?;
                        // Re-fetch model list so newly discovered custom models appear
                        list_available_models().await
                    },
                    |result| match result {
                        Ok(models) => cosmic::Action::App(Message::AvailableModelsLoaded(models)),
                        Err(e) => cosmic::Action::App(Message::CustomModelsDirError(e)),
                    },
                );
            }
            Message::CustomModelsDirError(err) => {
                log::warn!("Custom models dir error: {err}");
                return Task::none();
            }
            _ => {}
        }

        if matches!(
            message,
            Message::AllowOnlineModelsToggled(_)
                | Message::AllowOnlineModelsLoaded(_)
                | Message::AllowOnlineModelsError(_)
                | Message::OpenAIApiKeyChanged(_)
                | Message::OpenAIApiKeySaved
                | Message::OpenAIApiKeyRemoved
                | Message::OpenAIApiKeyError(_)
                | Message::OpenAIApiKeyStatusLoaded(_)
                | Message::MistralApiKeyChanged(_)
                | Message::MistralApiKeySaved
                | Message::MistralApiKeyRemoved
                | Message::MistralApiKeyError(_)
                | Message::MistralApiKeyStatusLoaded(_)
                | Message::DeepgramApiKeyChanged(_)
                | Message::DeepgramApiKeySaved
                | Message::DeepgramApiKeyRemoved
                | Message::DeepgramApiKeyError(_)
                | Message::DeepgramApiKeyStatusLoaded(_)
        ) {
            return self.handle_online_models_messages(message);
        }

        match message {
            // Original template messages
            Message::OpenRepositoryUrl => {
                _ = open::that_detached(views::about::REPOSITORY);
            }

            Message::ToggleContextPage(context_page) => {
                if self.context_page == context_page {
                    self.core.window.show_context = !self.core.window.show_context;
                } else {
                    self.context_page = context_page;
                    self.core.window.show_context = true;
                }

                // Refresh device info (including GPU free memory) when opening model drawer
                if context_page == ContextPage::ModelSelection && self.core.window.show_context {
                    return Task::perform(get_current_device(), |result| match result {
                        Ok((device, available_devices, gpu_memory)) => cosmic::Action::App(
                            Message::DeviceInfoLoaded(device, available_devices, gpu_memory),
                        ),
                        Err(e) => cosmic::Action::App(Message::DeviceError(e)),
                    });
                }
            }

            Message::LaunchUrl(url) => match open::that_detached(&url) {
                Ok(()) => {}
                Err(err) => {
                    eprintln!("failed to open {url:?}: {err}");
                }
            },

            // Super STT specific messages
            Message::StartRecording => {
                if matches!(self.recording_status, RecordingStatus::Recording) {
                    return Task::none();
                }

                self.recording_status = RecordingStatus::Recording;
                self.transcription_text.clear();

                let stream = record_command_stream();
                return cosmic::task::stream(stream.map(|event| match event {
                    RecordEvent::Preview(text) => {
                        cosmic::Action::App(Message::PreviewTextReceived(text))
                    }
                    RecordEvent::Final(Ok(text)) => {
                        cosmic::Action::App(Message::TranscriptionReceived(text))
                    }
                    RecordEvent::Final(Err(e)) => {
                        cosmic::Action::App(Message::TranscriptionReceived(format!("Error: {e}")))
                    }
                }));
            }

            Message::StopRecording => {
                return Task::perform(stop_record_command(), |result| {
                    if let Err(e) = result {
                        log::warn!("Stop recording failed: {e}");
                    }
                    cosmic::Action::None
                });
            }

            Message::PreviewTextReceived(text) => {
                self.transcription_text = text;
            }

            Message::TranscriptionReceived(text) => {
                log::info!(
                    "TranscriptionReceived: '{}'",
                    text.chars().take(50).collect::<String>()
                );
                self.transcription_text = text;
                self.recording_status = RecordingStatus::Idle;
                self.audio_level = 0.0;
            }

            Message::AudioLevelUpdate { level, is_speech } => {
                self.audio_level = level;
                self.is_speech_detected = is_speech;
            }

            Message::AudioFeedbackToggled(enabled) => {
                let theme = if enabled {
                    self.last_non_silent_theme
                } else {
                    AudioTheme::Silent
                };
                self.selected_audio_theme = theme;
                return Task::perform(set_audio_theme(theme), |result| match result {
                    Ok(_) => cosmic::Action::App(Message::DaemonConnected),
                    Err(e) => cosmic::Action::App(Message::DaemonError(e)),
                });
            }

            Message::AudioThemeSelected(theme) => {
                self.selected_audio_theme = theme;
                if theme != AudioTheme::Silent {
                    self.last_non_silent_theme = theme;
                }
                return Task::perform(set_and_test_audio_theme(theme), |result| match result {
                    Ok(_) => cosmic::Action::App(Message::DaemonConnected),
                    Err(e) => cosmic::Action::App(Message::DaemonError(e)),
                });
            }

            Message::SetAudioTheme(theme) => {
                self.selected_audio_theme = theme;
                // Audio theme preference is now saved by the daemon automatically
            }

            Message::AudioThemesLoaded(themes) => {
                self.audio_themes = themes;
            }

            Message::VolumeChanged(vol) => {
                self.volume = vol;
                return Task::perform(set_volume(vol), |result| match result {
                    Ok(()) => cosmic::Action::None,
                    Err(e) => cosmic::Action::App(Message::DaemonError(e)),
                });
            }

            Message::WidgetAudioLevel { level, is_speech } => {
                self.last_udp_data = std::time::Instant::now();
                self.audio_level = level;
                self.is_speech_detected = is_speech;
            }
            Message::WidgetRecordingState(is_recording) => {
                self.last_udp_data = std::time::Instant::now();
                self.recording_status = if is_recording {
                    RecordingStatus::Recording
                } else {
                    RecordingStatus::Idle
                };
            }

            Message::RecordingStateChanged(state) => {
                self.recording_status = state;
            }

            // Handled by helper methods
            _ => {}
        }
        Task::none()
    }

    /// Called when a nav item is selected.
    fn on_nav_select(&mut self, id: nav_bar::Id) -> Task<cosmic::Action<Self::Message>> {
        // Activate the page in the model.
        self.nav.activate(id);

        self.update_title()
    }
}

impl AppModel {
    /// Check if model is ready
    pub fn is_model_ready(&self) -> bool {
        matches!(self.model_operation_state, ModelOperationState::Ready)
    }

    /// Set model to downloading state
    pub fn set_model_downloading(
        &mut self,
        target_model: String,
        progress: super_stt_shared::models::protocol::DownloadProgress,
    ) {
        self.model_operation_state = ModelOperationState::Downloading {
            target_model,
            progress,
        };
    }

    /// Set model to loading state
    pub fn set_model_loading(&mut self, target_model: String, status_message: String) {
        self.model_operation_state = ModelOperationState::Loading {
            target_model,
            status_message,
        };
    }

    /// Set device to switching state
    pub fn set_device_switching(&mut self, target_device: String, status_message: String) {
        self.device_state = DeviceState::Switching {
            target_device,
            status_message,
        };
    }

    /// Handle daemon connection messages
    #[allow(clippy::too_many_lines)]
    fn handle_daemon_messages(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
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

            Message::DaemonConnected => {
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
                // — i.e. another potential keyring touch.
                if was_disconnected {
                    info!(
                        "Daemon reconnected; events subscription is self-healing, no iced restart"
                    );
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

                // Reload models, device info, and per-setting state on reconnect.
                // Each setting is fetched with its own dedicated GET call —
                // no bulk fetch_daemon_config anymore.
                let load_settings = Task::batch([
                    Task::perform(get_current_audio_theme(), |result| match result {
                        Ok(theme) => cosmic::Action::App(Message::CurrentAudioThemeLoaded(theme)),
                        Err(e) => {
                            warn!("Failed to load audio theme: {e}");
                            cosmic::Action::App(Message::CurrentAudioThemeLoaded(
                                AudioTheme::default(),
                            ))
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
                        Ok(enabled) => {
                            cosmic::Action::App(Message::PreviewTypingSettingLoaded(enabled))
                        }
                        Err(e) => {
                            log::warn!("Failed to load preview typing setting: {e}");
                            cosmic::Action::App(Message::PreviewTypingSettingLoaded(false))
                        }
                    }),
                    Task::perform(get_recording_stop_mode(), |result| {
                        use super_stt_shared::models::recording_stop_mode::RecordingStopMode;
                        match result {
                            Ok(mode_str) => {
                                let mode =
                                    mode_str.parse::<RecordingStopMode>().unwrap_or_default();
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
                                cosmic::Action::App(Message::WriteMethodLoaded(
                                    WriteMethod::default(),
                                ))
                            }
                        }
                    }),
                ]);

                if was_disconnected {
                    Task::batch([
                        self.handle_model_messages(Message::LoadInitialData),
                        load_settings,
                    ])
                } else {
                    load_settings
                }
            }

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

            Message::RefreshDaemonStatus => Task::perform(test_daemon_connection(), |result| {
                cosmic::Action::App(Message::DaemonConnectionResult(result))
            }),

            Message::PingTimeout => {
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

            Message::DaemonEventsReceived(events) => {
                info!("Received {} daemon events", events.len());
                for event in events {
                    // Update timestamp for next polling
                    self.last_event_timestamp = Some(event.timestamp.clone());

                    // Process device-related events
                    if event.event_type == "daemon_status_changed" {
                        info!("Received daemon event: {:?}", event.data);
                        if let Some(status) = event.data.get("status").and_then(|s| s.as_str()) {
                            match status {
                                // Note: "device_switched" event handler removed - we now only use "ready" events
                                // for device switch completion to ensure model is actually loaded
                                "ready" => {
                                    // Handle device readiness
                                    if let Some(actual_device) =
                                        event.data.get("actual_device").and_then(|d| d.as_str())
                                    {
                                        info!(
                                            "Received ready event: current_device={} -> {}",
                                            self.current_device, actual_device
                                        );
                                        self.current_device = actual_device.to_string();

                                        // If we were switching devices, this marks completion
                                        if matches!(
                                            self.device_state,
                                            DeviceState::Switching { .. }
                                        ) {
                                            info!("Device switch completed to: {actual_device}");
                                        }
                                        self.device_state = DeviceState::Ready;
                                    }

                                    // Handle model readiness - clear switching state
                                    if event
                                        .data
                                        .get("model_loaded")
                                        .and_then(serde_json::Value::as_bool)
                                        .unwrap_or(false)
                                    {
                                        info!("Received ready event: model loading completed");
                                        info!(
                                            "Model state before ready event: {:?}",
                                            self.model_operation_state
                                        );
                                        self.model_operation_state = ModelOperationState::Ready;
                                        info!(
                                            "Model state after ready event: {:?}",
                                            self.model_operation_state
                                        );
                                    }
                                }
                                "device_switch_error" | "error" => {
                                    warn!("Received device switch error event: {:?}", event.data);
                                    // Reset device state from switching to ready
                                    if matches!(self.device_state, DeviceState::Switching { .. }) {
                                        info!("Device switch failed, reverting to ready state");
                                    }
                                    self.device_state = DeviceState::Ready;
                                    if let Some(error_msg) =
                                        event.data.get("error").and_then(|e| e.as_str())
                                    {
                                        let error_message = error_msg.to_string();
                                        // Show error to user
                                        return Task::perform(
                                            async move { error_message },
                                            |msg| cosmic::Action::App(Message::DeviceError(msg)),
                                        );
                                    }
                                }
                                "model_switched" => {
                                    if let Some(model_name) =
                                        event.data.get("model_name").and_then(|m| m.as_str())
                                    {
                                        let model = model_name.to_string();
                                        let provider = event
                                            .data
                                            .get("provider")
                                            .and_then(|p| p.as_str())
                                            .and_then(|s| s.parse::<Provider>().ok())
                                            .unwrap_or_else(|| {
                                                registry::find(&model)
                                                    .map_or(self.current_provider, |d| d.provider)
                                            });
                                        let source = event
                                            .data
                                            .get("source")
                                            .and_then(|p| p.as_str())
                                            .and_then(|s| s.parse::<SourceKind>().ok())
                                            .unwrap_or(self.current_source);
                                        info!(
                                            "Received model_switched event: current_model={:?} -> {:?} via {provider} ({source})",
                                            self.current_model, model
                                        );
                                        self.current_model = model;
                                        self.current_provider = provider;
                                        self.current_source = source;
                                        self.model_operation_state = ModelOperationState::Ready;
                                        info!(
                                            "Model state updated to Ready after model_switched event"
                                        );
                                    }
                                }
                                "switching_device" => {
                                    info!("Received switching_device event: {:?}", event.data);
                                    // Keep device_state as Switching and wait for "ready" event
                                    // This event just confirms the switch is in progress
                                    if !matches!(self.device_state, DeviceState::Switching { .. }) {
                                        warn!(
                                            "Received switching_device event but not in switching state"
                                        );
                                        if let Some(to_device) =
                                            event.data.get("to_device").and_then(|d| d.as_str())
                                        {
                                            self.set_device_switching(
                                                to_device.to_string(),
                                                "Switching device...".to_string(),
                                            );
                                        }
                                    }
                                }
                                "loading_model_for_device" => {
                                    info!(
                                        "Received loading_model_for_device event: {:?}",
                                        event.data
                                    );
                                    if let (Some(target_device), Some(model)) = (
                                        event.data.get("target_device").and_then(|d| d.as_str()),
                                        event.data.get("model").and_then(|m| m.as_str()),
                                    ) {
                                        let status_message = format!(
                                            "Loading {} on {}...",
                                            model,
                                            if target_device == "cpu" { "CPU" } else { "GPU" }
                                        );
                                        self.set_device_switching(
                                            target_device.to_string(),
                                            status_message,
                                        );
                                    }
                                }
                                _ => {
                                    info!("Received unhandled daemon status: {status}");
                                }
                            }
                        }
                    } else if event.event_type == "download_progress" {
                        // Handle download progress events
                        if let Ok(progress) = serde_json::from_value::<
                            super_stt_shared::models::protocol::DownloadProgress,
                        >(event.data.clone())
                        {
                            info!(
                                "Received download progress event: {}% for {}",
                                progress.percentage, progress.model_name
                            );
                            // Determine target model from progress data
                            {
                                let target_model = progress.model_name.clone();
                                match progress.status.as_str() {
                                    "loading_model" => {
                                        self.set_model_loading(
                                            target_model,
                                            "Loading model into memory...".to_string(),
                                        );
                                    }
                                    "completed" | "cancelled" | "error" => {
                                        // State will be updated by subsequent daemon events (model_switched, ready, etc.)
                                        info!(
                                            "Download completed with status: {}",
                                            progress.status
                                        );
                                    }
                                    _ => {
                                        // "downloading" and other states default to downloading
                                        self.set_model_downloading(target_model, progress.clone());
                                    }
                                }
                            }

                            // Handle download completion/failure
                            if progress.status == "completed" {
                                // Send download completed message and reload models after a brief delay
                                return Task::batch([
                                    Task::perform(async move { progress.model_name }, |model| {
                                        cosmic::Action::App(Message::DownloadCompleted(model))
                                    }),
                                    Task::none(), // Model reload not needed - daemon will broadcast model_switched event if needed
                                ]);
                            } else if progress.status == "cancelled" {
                                return Task::perform(
                                    async move { progress.model_name },
                                    |model| cosmic::Action::App(Message::DownloadCancelled(model)),
                                );
                            } else if progress.status == "error" {
                                let error_msg =
                                    format!("Download failed for {}", progress.model_name);
                                return Task::perform(
                                    async move { (progress.model_name, error_msg) },
                                    |(model, error)| {
                                        cosmic::Action::App(Message::DownloadError { model, error })
                                    },
                                );
                            }
                        }
                    }
                }
                // Force UI update after processing events that may change state
                self.update_title()
            }

            Message::DaemonEventsError(error) => {
                warn!("Daemon events error: {error}");
                // Log the error but continue - subscription will retry automatically
                Task::none()
            }

            _ => Task::none(),
        }
    }

    /// Handle device management messages
    fn handle_device_messages(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::DeviceSelected(device) => {
                if device != self.current_device && self.device_state == DeviceState::Ready {
                    self.set_device_switching(device.clone(), "Switching device...".to_string());
                    self.last_device_switch = Some(std::time::Instant::now());

                    info!("Switching to device: {device}");
                    let target_device = device.clone();
                    Task::perform(
                        async move {
                            // Send device switch command and trust the daemon's response
                            match set_device(target_device.clone()).await {
                                Ok(()) => {
                                    // Device switch command succeeded - assume the target device is now active
                                    // We don't verify with get_device to avoid premature requests
                                    info!("Device switch command completed successfully");
                                    Ok(target_device)
                                }
                                Err(e) => Err(e),
                            }
                        },
                        |result| match result {
                            Ok(_device) => {
                                // Don't simulate DeviceInfoLoaded - wait for daemon's "ready" event
                                // to confirm the device switch is actually complete
                                info!(
                                    "Device switch command sent successfully, waiting for daemon confirmation"
                                );
                                cosmic::Action::None
                            }
                            Err(e) => cosmic::Action::App(Message::DeviceError(e)),
                        },
                    )
                } else if matches!(self.device_state, DeviceState::Switching { .. }) {
                    warn!("Device switch already in progress - ignoring");
                    Task::none()
                } else {
                    Task::none()
                }
            }

            Message::DeviceLoaded(device) => {
                self.current_device = device;
                self.device_state = DeviceState::Ready;
                Task::none()
            }

            Message::DeviceInfoLoaded(device, available_devices, gpu_memory) => {
                info!("DeviceInfoLoaded: device={device}, available_devices={available_devices:?}");
                self.current_device.clone_from(&device);
                self.available_devices.clone_from(&available_devices);
                self.gpu_memory = gpu_memory;

                if matches!(self.device_state, DeviceState::Switching { .. }) {
                    info!("Device switch completed to: {device}");
                    self.device_state = DeviceState::Cooldown;
                    // No need to reload models - device switch complete and model state maintained via events
                    Task::none()
                } else {
                    self.device_state = DeviceState::Ready;
                    Task::none()
                }
            }

            Message::DeviceError(err) => {
                self.device_state = DeviceState::Ready;
                self.transcription_text = format!("Device Error: {err}");
                Task::none()
            }

            _ => Task::none(),
        }
    }

    /// Handle download progress messages
    #[allow(clippy::too_many_lines)]
    fn handle_download_messages(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::DownloadProgressUpdate(progress) => {
                // We have an actual download in progress
                {
                    let target_model = progress.model_name.clone();
                    match progress.status.as_str() {
                        "loading_model" => {
                            self.set_model_loading(
                                target_model,
                                "Loading model into memory...".to_string(),
                            );
                        }
                        "completed" | "cancelled" | "error" => {
                            // State will be updated by subsequent daemon events
                            info!("Download completed with status: {}", progress.status);
                        }
                        _ => {
                            // "downloading" and other states default to downloading
                            self.set_model_downloading(target_model, progress);
                        }
                    }
                }

                Task::none()
            }

            Message::CancelDownload => Task::perform(cancel_download(), |result| match result {
                Ok(_) => cosmic::Action::App(Message::DownloadCancelled(String::new())),
                Err(e) => cosmic::Action::App(Message::DownloadError {
                    model: String::new(),
                    error: e,
                }),
            }),

            Message::DownloadCompleted(model_name) => {
                info!("Model {model_name} finished downloading");
                // Model information will be updated via daemon events (model_switched, ready)
                Task::none()
            }

            Message::DownloadCancelled(model_name) => {
                info!("Model {model_name} download was cancelled");
                self.model_operation_state = ModelOperationState::Ready;

                // Revert to previous model
                let previous_model = self.previous_model.clone();
                let previous_provider = self.previous_provider;
                let previous_source = self.previous_source;
                Task::perform(
                    set_model(
                        self.previous_model.clone(),
                        self.previous_provider,
                        self.previous_source,
                    ),
                    move |result| match result {
                        Ok(_) => cosmic::Action::App(Message::ModelChanged {
                            model: previous_model.clone(),
                            provider: previous_provider,
                            source: previous_source,
                        }),
                        Err(e) => cosmic::Action::App(Message::ModelError(e)),
                    },
                )
            }

            Message::DownloadError { model, error } => {
                warn!("Download error for model {model}: {error}");
                self.model_operation_state = ModelOperationState::Ready;
                self.transcription_text = format!("Download Error: {error}");

                // Revert to previous model
                let previous_model = self.previous_model.clone();
                let previous_provider = self.previous_provider;
                let previous_source = self.previous_source;
                Task::perform(
                    set_model(
                        self.previous_model.clone(),
                        self.previous_provider,
                        self.previous_source,
                    ),
                    move |result| match result {
                        Ok(_) => cosmic::Action::App(Message::ModelChanged {
                            model: previous_model.clone(),
                            provider: previous_provider,
                            source: previous_source,
                        }),
                        Err(e) => cosmic::Action::App(Message::ModelError(e)),
                    },
                )
            }

            Message::CheckDownloadStatus => {
                // Check download status if model is not ready
                if self.is_model_ready() {
                    Task::none()
                } else {
                    Task::perform(get_download_status(), |result| match result {
                        Ok(Some(progress)) => {
                            // Download is actually happening
                            cosmic::Action::App(Message::DownloadProgressUpdate(progress))
                        }
                        Ok(None) => {
                            // No download in progress, model must have loaded from cache
                            cosmic::Action::App(Message::NoDownloadInProgress)
                        }
                        Err(_) => {
                            // Failed to get status, assume no download
                            cosmic::Action::App(Message::NoDownloadInProgress)
                        }
                    })
                }
            }

            Message::NoDownloadInProgress => {
                // Clear state since there's no active download - set to ready
                self.model_operation_state = ModelOperationState::Ready;
                // Model state is already maintained via events, no reload needed
                Task::none()
            }

            _ => Task::none(),
        }
    }

    /// Handle model management messages
    #[allow(clippy::too_many_lines)]
    fn handle_model_messages(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::LoadInitialData => {
                info!("LoadInitialData: Loading models and device info at startup");
                // One-time startup load: models + device info
                Task::batch([
                    Task::perform(list_available_models(), |result| match result {
                        Ok(models) => cosmic::Action::App(Message::AvailableModelsLoaded(models)),
                        Err(e) => cosmic::Action::App(Message::ModelError(e)),
                    }),
                    Task::perform(get_current_model(), |result| match result {
                        Ok((model, provider, source)) => {
                            cosmic::Action::App(Message::CurrentModelLoaded {
                                model,
                                provider,
                                source,
                            })
                        }
                        Err(e) => cosmic::Action::App(Message::ModelError(e)),
                    }),
                    Task::perform(get_current_device(), |result| match result {
                        Ok((device, available_devices, gpu_memory)) => {
                            info!(
                                "Initial device load successful: device={device}, available_devices={available_devices:?}"
                            );
                            cosmic::Action::App(Message::DeviceInfoLoaded(
                                device,
                                available_devices,
                                gpu_memory,
                            ))
                        }
                        Err(e) => {
                            warn!("Initial device load failed: {e}");
                            cosmic::Action::App(Message::DeviceError(e))
                        }
                    }),
                    Task::perform(get_allow_online_models(), |result| match result {
                        Ok(enabled) => {
                            cosmic::Action::App(Message::AllowOnlineModelsLoaded(enabled))
                        }
                        Err(e) => cosmic::Action::App(Message::AllowOnlineModelsError(e)),
                    }),
                    Task::perform(
                        async { crate::keyring::has_api_key("openai").unwrap_or(false) },
                        |has_key| cosmic::Action::App(Message::OpenAIApiKeyStatusLoaded(has_key)),
                    ),
                    Task::perform(
                        async { crate::keyring::has_api_key("mistral").unwrap_or(false) },
                        |has_key| cosmic::Action::App(Message::MistralApiKeyStatusLoaded(has_key)),
                    ),
                    Task::perform(
                        async { crate::keyring::has_api_key("deepgram").unwrap_or(false) },
                        |has_key| cosmic::Action::App(Message::DeepgramApiKeyStatusLoaded(has_key)),
                    ),
                ])
            }

            Message::ModelSelected {
                model,
                provider,
                source,
            } => {
                // Close the model selection drawer and clear search
                self.core.window.show_context = false;
                self.model_search.clear();

                if model == self.current_model
                    && provider == self.current_provider
                    && source == self.current_source
                {
                    Task::none()
                } else {
                    // Atomic state check and transition to prevent race conditions
                    if !self.is_model_ready() {
                        warn!("Model operation already in progress - ignoring concurrent request");
                        return Task::none();
                    }

                    // Set loading state for the target model
                    self.set_model_loading(model.clone(), "Initiating model switch...".to_string());

                    // Save the current model as previous (to revert to on cancel)
                    self.previous_model = self.current_model.clone();
                    self.previous_provider = self.current_provider;
                    self.previous_source = self.current_source;

                    let selected_model = model.clone();
                    Task::batch([
                        Task::perform(
                            set_model(model, provider, source),
                            move |result| match result {
                                Ok(_) => cosmic::Action::App(Message::ModelChanged {
                                    model: selected_model.clone(),
                                    provider,
                                    source,
                                }),
                                Err(e) => cosmic::Action::App(Message::ModelError(e)),
                            },
                        ),
                        // Check download status immediately to see if download is needed
                        Task::perform(
                            async move {
                                // Small delay to allow daemon to start download if needed
                                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                            },
                            |()| cosmic::Action::App(Message::CheckDownloadStatus),
                        ),
                    ])
                }
            }

            Message::ModelsLoaded {
                current_model,
                current_provider,
                current_source,
                available,
            } => {
                self.available_models = available;
                self.current_model = current_model;
                self.current_provider = current_provider;
                self.current_source = current_source;

                // Set model to ready state
                self.model_operation_state = ModelOperationState::Ready;

                Task::none()
            }

            Message::AvailableModelsLoaded(models) => {
                self.available_models = models;
                Task::none()
            }

            Message::CurrentModelLoaded {
                model,
                provider,
                source,
            }
            | Message::ModelChanged {
                model,
                provider,
                source,
            } => {
                self.current_model = model;
                self.current_provider = provider;
                self.current_source = source;
                self.model_operation_state = ModelOperationState::Ready;
                Task::none()
            }

            Message::ModelError(err) => {
                warn!("Model operation failed: {err}");
                let sanitized = err
                    .replace(&std::env::var("HOME").unwrap_or_default(), "$HOME")
                    .chars()
                    .take(200)
                    .collect::<String>();
                self.model_operation_state = ModelOperationState::Error { message: sanitized };
                Task::none()
            }

            _ => Task::none(),
        }
    }

    /// Handle preview typing messages
    fn handle_preview_typing_messages(
        &mut self,
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            Message::PreviewTypingToggled(enabled) => {
                self.preview_typing_enabled = enabled;
                Task::perform(set_preview_typing(enabled), move |result| match result {
                    Ok(()) => cosmic::Action::App(Message::PreviewTypingSettingLoaded(enabled)),
                    Err(e) => cosmic::Action::App(Message::PreviewTypingError(e)),
                })
            }

            Message::PreviewTypingSettingLoaded(enabled) => {
                self.preview_typing_enabled = enabled;
                Task::none()
            }

            Message::PreviewTypingError(err) => {
                // Log error and show it to user in transcription text
                log::warn!("Preview typing error: {err}");
                self.transcription_text = format!("Preview Typing Error: {err}");
                Task::none()
            }

            _ => Task::none(),
        }
    }

    /// Handle recording stop mode messages
    fn handle_recording_stop_mode_messages(
        &mut self,
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            Message::RecordingStopModeChanged(mode) => {
                self.recording_stop_mode = mode;
                let mode_str = mode.to_string();
                Task::perform(
                    set_recording_stop_mode(mode_str),
                    move |result| match result {
                        Ok(()) => cosmic::Action::App(Message::RecordingStopModeLoaded(mode)),
                        Err(e) => cosmic::Action::App(Message::RecordingStopModeError(e)),
                    },
                )
            }

            Message::RecordingStopModeLoaded(mode) => {
                self.recording_stop_mode = mode;
                Task::none()
            }

            Message::RecordingStopModeError(err) => {
                log::warn!("Recording stop mode error: {err}");
                Task::none()
            }

            _ => Task::none(),
        }
    }

    /// Handle write method messages
    fn handle_write_method_messages(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::WriteMethodChanged(method) => {
                self.write_method = method;
                let method_str = method.to_string();
                Task::perform(set_write_method(method_str), move |result| match result {
                    Ok(()) => cosmic::Action::App(Message::WriteMethodLoaded(method)),
                    Err(e) => cosmic::Action::App(Message::WriteMethodError(e)),
                })
            }

            Message::WriteMethodLoaded(method) => {
                self.write_method = method;
                Task::none()
            }

            Message::WriteMethodError(err) => {
                log::warn!("Write method error: {err}");
                Task::none()
            }

            _ => Task::none(),
        }
    }

    /// Handle online models messages
    #[allow(clippy::too_many_lines)]
    fn handle_online_models_messages(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::AllowOnlineModelsToggled(enabled) => {
                self.allow_online_models = enabled;
                Task::perform(
                    set_allow_online_models(enabled),
                    move |result| match result {
                        Ok(()) => cosmic::Action::App(Message::AllowOnlineModelsLoaded(enabled)),
                        Err(e) => cosmic::Action::App(Message::AllowOnlineModelsError(e)),
                    },
                )
            }

            Message::AllowOnlineModelsLoaded(enabled) => {
                self.allow_online_models = enabled;
                Task::none()
            }

            Message::AllowOnlineModelsError(err) => {
                log::warn!("Allow online models error: {err}");
                Task::none()
            }

            Message::OpenAIApiKeyChanged(key) => {
                self.openai_api_key_input = key;
                Task::none()
            }

            Message::OpenAIApiKeySaved => {
                let key = self.openai_api_key_input.clone();
                if key.is_empty() {
                    return Task::none();
                }
                Task::perform(
                    async move { crate::keyring::set_api_key("openai", &key) },
                    |result| match result {
                        Ok(()) => cosmic::Action::App(Message::OpenAIApiKeyStatusLoaded(true)),
                        Err(e) => cosmic::Action::App(Message::OpenAIApiKeyError(e)),
                    },
                )
            }

            Message::OpenAIApiKeyRemoved => Task::perform(
                async { crate::keyring::delete_api_key("openai") },
                |result| match result {
                    Ok(()) => cosmic::Action::App(Message::OpenAIApiKeyStatusLoaded(false)),
                    Err(e) => cosmic::Action::App(Message::OpenAIApiKeyError(e)),
                },
            ),

            Message::OpenAIApiKeyError(err) => {
                log::warn!("OpenAI API key error: {err}");
                Task::none()
            }

            Message::OpenAIApiKeyStatusLoaded(has_key) => {
                self.has_openai_api_key = has_key;
                if has_key {
                    self.openai_api_key_input.clear();
                }
                Task::none()
            }

            Message::MistralApiKeyChanged(key) => {
                self.mistral_api_key_input = key;
                Task::none()
            }

            Message::MistralApiKeySaved => {
                let key = self.mistral_api_key_input.clone();
                if key.is_empty() {
                    return Task::none();
                }
                Task::perform(
                    async move { crate::keyring::set_api_key("mistral", &key) },
                    |result| match result {
                        Ok(()) => cosmic::Action::App(Message::MistralApiKeyStatusLoaded(true)),
                        Err(e) => cosmic::Action::App(Message::MistralApiKeyError(e)),
                    },
                )
            }

            Message::MistralApiKeyRemoved => Task::perform(
                async { crate::keyring::delete_api_key("mistral") },
                |result| match result {
                    Ok(()) => cosmic::Action::App(Message::MistralApiKeyStatusLoaded(false)),
                    Err(e) => cosmic::Action::App(Message::MistralApiKeyError(e)),
                },
            ),

            Message::MistralApiKeyError(err) => {
                log::warn!("Mistral API key error: {err}");
                Task::none()
            }

            Message::MistralApiKeyStatusLoaded(has_key) => {
                self.has_mistral_api_key = has_key;
                if has_key {
                    self.mistral_api_key_input.clear();
                }
                Task::none()
            }

            Message::DeepgramApiKeyChanged(key) => {
                self.deepgram_api_key_input = key;
                Task::none()
            }

            Message::DeepgramApiKeySaved => {
                let key = self.deepgram_api_key_input.clone();
                if key.is_empty() {
                    return Task::none();
                }
                Task::perform(
                    async move { crate::keyring::set_api_key("deepgram", &key) },
                    |result| match result {
                        Ok(()) => cosmic::Action::App(Message::DeepgramApiKeyStatusLoaded(true)),
                        Err(e) => cosmic::Action::App(Message::DeepgramApiKeyError(e)),
                    },
                )
            }

            Message::DeepgramApiKeyRemoved => Task::perform(
                async { crate::keyring::delete_api_key("deepgram") },
                |result| match result {
                    Ok(()) => cosmic::Action::App(Message::DeepgramApiKeyStatusLoaded(false)),
                    Err(e) => cosmic::Action::App(Message::DeepgramApiKeyError(e)),
                },
            ),

            Message::DeepgramApiKeyError(err) => {
                log::warn!("Deepgram API key error: {err}");
                Task::none()
            }

            Message::DeepgramApiKeyStatusLoaded(has_key) => {
                self.has_deepgram_api_key = has_key;
                if has_key {
                    self.deepgram_api_key_input.clear();
                }
                Task::none()
            }

            _ => Task::none(),
        }
    }

    /// Updates the header and window titles.
    pub fn update_title(&mut self) -> Task<cosmic::Action<Message>> {
        let mut window_title = "Super STT".to_string();

        if let Some(page) = self.nav.text(self.nav.active()) {
            window_title.push_str(" — ");
            window_title.push_str(page);
        }

        if let Some(id) = self.core.main_window_id() {
            self.set_window_title(window_title, id)
        } else {
            Task::none()
        }
    }
}

/// Classify a daemon error message into the right next `DaemonStatus`.
///
/// `auth_denied (user_denied)` / `auth_denied (user_denied_cached)`
/// must transition to [`DaemonStatus::Blocked`] so the surrounding
/// auto-retry loop in `Message::DaemonError` stops firing — otherwise
/// the settings app re-pings every 5s and the daemon's in-memory
/// deny cache keeps logging the same `user_denied_cached`. Any other
/// error string is transient (daemon restarting, socket missing,
/// etc.) and gets [`DaemonStatus::Error`], which the caller pairs
/// with the 5s retry. Extracted out of `Message::DaemonError` so
/// this load-bearing branch is unit-testable without dragging the
/// full cosmic update loop into the test harness.
fn classify_daemon_error(err: String) -> DaemonStatus {
    if super_stt_shared::daemon::widget_subscription::is_user_denied(&err) {
        DaemonStatus::Blocked(err)
    } else {
        DaemonStatus::Error(err)
    }
}

/// Wrapper for `Subscription::run_with` so the subscription restarts when the counter changes.
#[derive(Hash)]
struct UdpSubscriptionId(u64);

/// Subscribe to the daemon's `/events` SSE stream for the settings UI's
/// audio meter and recording-state indicator. Reuses the
/// settings-scope token that's already cached for normal config calls
/// — settings is god-mode so it can subscribe to widget topics.
const SETTINGS_APP_ID: super_stt_shared::daemon::session::AppId =
    super_stt_shared::daemon::session::AppId("super-stt-app");
const SETTINGS_APP_NAME: &str = "Super STT Settings App";
const SETTINGS_SCOPE: &str = "settings";
const SETTINGS_AUDIO_TOPICS: &[&str] = &["recording_state", "frequency_bands"];

/// Self-healing `/events` subscription for the settings UI's audio
/// meter + recording-status badge. Routes through the shared
/// [`run_widget_subscription`] helper so silent drops, idle wedges,
/// and daemon-side revocations all auto-recover with backoff.
fn audio_events_subscription(
    _id: &UdpSubscriptionId,
) -> std::pin::Pin<Box<dyn cosmic::iced::futures::Stream<Item = Message> + Send>> {
    use futures_util::StreamExt;
    use super_stt_shared::daemon::widget_subscription::{
        WidgetSubscriptionConfig, WidgetSubscriptionUpdate, run_widget_subscription,
    };
    use super_stt_shared::validation::get_http_socket_path;

    Box::pin(cosmic::iced::stream::channel(100, async |mut channel| {
        let config = WidgetSubscriptionConfig::new(
            SETTINGS_APP_ID,
            SETTINGS_APP_NAME,
            SETTINGS_SCOPE,
            SETTINGS_AUDIO_TOPICS,
        );
        let mut updates = Box::pin(run_widget_subscription(get_http_socket_path(), config));
        info!("Settings subscription starting");
        while let Some(update) = updates.next().await {
            let msg = match update {
                WidgetSubscriptionUpdate::Connected => continue,
                WidgetSubscriptionUpdate::Event(evt) => {
                    match settings_widget_event_to_message(&evt) {
                        Some(m) => m,
                        None => continue,
                    }
                }
                WidgetSubscriptionUpdate::Disconnected { reason } => {
                    warn!("Settings /events disconnected ({reason}); reconnecting");
                    continue;
                }
                WidgetSubscriptionUpdate::NeedsReauth { reason } => {
                    warn!(
                        "Settings session needs re-auth ({reason}); will re-consent on next attempt"
                    );
                    continue;
                }
                WidgetSubscriptionUpdate::Blocked { reason } => {
                    warn!("Settings session blocked by user denial ({reason}); subscription ended");
                    // Forward to the update loop so the UI flips to
                    // the Blocked state (Retry button) instead of
                    // sitting silently with a dead audio meter.
                    Message::WidgetBlocked(reason)
                }
            };
            if channel.send(msg).await.is_err() {
                break;
            }
        }
        info!("Settings subscription ended");
    }))
}

/// Pick out the events the settings UI cares about and translate them
/// into the `Message` variants that drive the audio meter +
/// recording-status badge. Returns `None` for events we don't render
/// (e.g. `subscribed`, `error`, `revoked`).
fn settings_widget_event_to_message(
    evt: &super_stt_shared::daemon::http_client::WidgetEvent,
) -> Option<Message> {
    use serde_json::Value;
    let p: &Value = &evt.payload;
    match evt.name.as_str() {
        "recording_state" => Some(Message::WidgetRecordingState(
            p.get("is_recording")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        )),
        "frequency_bands" => {
            #[allow(clippy::cast_possible_truncation)]
            let total_energy = p.get("total_energy").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let level = raw_level_to_db_display_percent(total_energy);
            let is_speech = total_energy > 0.0001;
            Some(Message::WidgetAudioLevel { level, is_speech })
        }
        _ => None,
    }
}

/// Convert raw frequency-band energy (typically 0.00001-0.1) into a
/// 0.0-1.0 display percentage via a -60 dB ... 0 dB log mapping. Same
/// transform the legacy UDP path applied in `audio/networking.rs`.
fn raw_level_to_db_display_percent(raw_level: f32) -> f32 {
    let db = if raw_level <= 0.0 {
        -60.0
    } else {
        // Same scaling used in the legacy UDP path: map quiet/normal/loud
        // speech (0.003 / 0.005 / 0.008) to ~80-97% display.
        (20.0 * (raw_level * 10.0).log10()).clamp(-60.0, 0.0)
    };
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
}

/// Wrapper for `Subscription::run_with` to avoid passing `&PathBuf` directly.
#[derive(Hash)]
struct DaemonSocketPath(PathBuf);

/// Creates a persistent subscription to daemon events
/// This maintains a persistent connection to receive real-time event notifications
fn daemon_event_subscription(
    config: &DaemonSocketPath,
) -> std::pin::Pin<Box<dyn cosmic::iced::futures::Stream<Item = Message> + Send>> {
    let socket_path = config.0.clone();
    Box::pin(cosmic::iced::stream::channel(
        100,
        async move |mut channel| {
            info!("Starting daemon event subscription loop");

            loop {
                info!("Attempting to establish persistent event connection");

                // Try to establish persistent connection to daemon for event streaming
                match create_persistent_event_connection(&socket_path, &mut channel).await {
                    Ok(()) => {
                        info!("Persistent event connection completed, will restart if needed");
                    }
                    Err(e) => {
                        warn!("Persistent event connection failed: {e}, retrying in 5 seconds");
                        let _ = channel.send(Message::DaemonEventsError(e)).await;

                        // Wait before retrying
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }

                // Brief pause before retrying connection
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        },
    ))
}

/// Creates a persistent connection for receiving real-time events from daemon
async fn create_persistent_event_connection<T>(
    socket_path: &PathBuf,
    channel: &mut T,
) -> Result<(), String>
where
    T: futures_util::SinkExt<Message> + Unpin,
{
    use super_stt_shared::models::protocol::{DaemonRequest, DaemonResponse};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;

    // Connect to daemon
    let mut stream = UnixStream::connect(socket_path)
        .await
        .map_err(|e| format!("Failed to connect to daemon: {e}"))?;

    info!("Connected to daemon for persistent event subscription");

    // Create subscription request
    let request = DaemonRequest {
        command: "subscribe".to_string(),
        event_types: Some(vec![
            "daemon_status_changed".to_string(),
            "download_progress".to_string(),
        ]),
        client_info: Some(std::collections::HashMap::new()),
        client_id: Some("super-stt-app-events".to_string()),
        data: None,
        audio_data: None,
        sample_rate: None,
        since_timestamp: None,
        limit: None,
        event_type: None,
        language: None,
        enabled: None,
    };

    // Serialize and send subscription request
    let request_data = serde_json::to_vec(&request)
        .map_err(|e| format!("Failed to serialize subscription request: {e}"))?;

    // Send size header + request
    let size = request_data.len() as u64;
    stream
        .write_all(&size.to_be_bytes())
        .await
        .map_err(|e| format!("Failed to write request size: {e}"))?;
    stream
        .write_all(&request_data)
        .await
        .map_err(|e| format!("Failed to write subscription request: {e}"))?;

    // Read initial response
    let mut size_buf = [0u8; 8];
    stream
        .read_exact(&mut size_buf)
        .await
        .map_err(|e| format!("Failed to read response size: {e}"))?;

    let response_size = u64::from_be_bytes(size_buf);
    let response_len = usize::try_from(response_size)
        .map_err(|_| "Response too large for this platform".to_string())?;

    let mut response_buf = vec![0u8; response_len];
    stream
        .read_exact(&mut response_buf)
        .await
        .map_err(|e| format!("Failed to read response: {e}"))?;

    // Parse initial response
    let response: DaemonResponse = serde_json::from_slice(&response_buf)
        .map_err(|e| format!("Failed to parse subscription response: {e}"))?;

    if response.status != "success" {
        return Err(format!(
            "Subscription failed: {}",
            response
                .message
                .unwrap_or_else(|| "Unknown error".to_string())
        ));
    }

    info!("Successfully subscribed to daemon events, entering streaming mode");

    // Now continuously read streamed events
    stream_daemon_events(stream, channel).await?;

    Ok(())
}

/// Continuously read and process streamed events from the daemon
async fn stream_daemon_events<T>(
    mut stream: tokio::net::UnixStream,
    channel: &mut T,
) -> Result<(), String>
where
    T: futures_util::SinkExt<Message> + Unpin,
{
    use tokio::io::AsyncReadExt;

    loop {
        // Read event size
        let mut size_buf = [0u8; 8];
        match stream.read_exact(&mut size_buf).await {
            Ok(_) => {}
            Err(e) => {
                warn!("Connection closed or error reading event size: {e}");
                break;
            }
        }

        let event_size = u64::from_be_bytes(size_buf);
        let Ok(event_len) = usize::try_from(event_size) else {
            warn!("Event too large, skipping");
            continue;
        };

        // Read event data
        let mut event_buf = vec![0u8; event_len];
        match stream.read_exact(&mut event_buf).await {
            Ok(_) => {}
            Err(e) => {
                warn!("Error reading event data: {e}");
                break;
            }
        }

        // Parse event
        match serde_json::from_slice::<super_stt_shared::models::protocol::NotificationEvent>(
            &event_buf,
        ) {
            Ok(event) => {
                if channel
                    .send(Message::DaemonEventsReceived(vec![event]))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Err(e) => {
                warn!("Failed to parse streamed event: {e}");
                // Continue processing other events
            }
        }
    }

    Ok(())
}

impl menu::action::MenuAction for MenuAction {
    type Message = Message;

    fn message(&self) -> Self::Message {
        match self {
            MenuAction::About => Message::ToggleContextPage(ContextPage::About),
        }
    }
}

#[cfg(test)]
mod classify_daemon_error_tests {
    //! Pin the decision in `Message::DaemonError`: which error
    //! strings should suppress the 5s auto-retry vs. trigger it.
    //! Locking this in protects against a regression of the deny
    //! spam loop the helper change already fixed for the applet.
    use super::*;

    #[test]
    fn user_denied_cached_routes_to_blocked() {
        // Verbatim daemon response shape — see
        // super-stt-daemon/src/daemon/http_server.rs::auth_err.
        let next = classify_daemon_error("auth_denied (user_denied_cached)".to_string());
        match next {
            DaemonStatus::Blocked(reason) => {
                assert_eq!(reason, "auth_denied (user_denied_cached)");
            }
            other => panic!("user_denied_cached must route to Blocked, got {other:?}"),
        }
    }

    #[test]
    fn fresh_user_denied_routes_to_blocked() {
        let next = classify_daemon_error("auth_denied (user_denied)".to_string());
        assert!(matches!(next, DaemonStatus::Blocked(_)));
    }

    #[test]
    fn dismissed_popup_routes_to_error_so_retry_can_recover() {
        // user_dismissed is recoverable — next attempt pops the
        // popup fresh — so the retry loop must keep firing.
        let next = classify_daemon_error("auth_denied (user_dismissed)".to_string());
        assert!(matches!(next, DaemonStatus::Error(_)));
    }

    #[test]
    fn invalid_session_routes_to_error() {
        // Token expiry / exe_changed are transient — let the
        // retry loop drive a fresh consent on the next attempt.
        let next = classify_daemon_error("invalid_session (expired)".to_string());
        assert!(matches!(next, DaemonStatus::Error(_)));
    }

    #[test]
    fn socket_unreachable_routes_to_error() {
        // Daemon restarting / socket missing — pure transient.
        let next = classify_daemon_error(
            "Daemon HTTP listener not running. Start the daemon first.".to_string(),
        );
        assert!(matches!(next, DaemonStatus::Error(_)));
    }
}
