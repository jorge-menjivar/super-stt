// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::client::{load_audio_themes, ping_daemon};
use crate::state::{AudioTheme, ContextPage, DaemonStatus, ModelsTab, RecordingStatus};
use crate::ui::icons;
use crate::ui::messages::{DaemonMessage, Message, ModelMessage, RecordingMessage};
use cosmic::prelude::*;
use cosmic::widget::{nav_bar, segmented_button};
use std::collections::HashMap;
use super_stt_shared::models::provider::Provider;

use super::{AppModel, DeviceState, ModelOperationState};

/// Builds the navigation bar with all Super STT pages inserted in order.
fn build_nav() -> nav_bar::Model {
    let mut nav = nav_bar::Model::default();

    // Models is the primary page (the active backend) — first in the rail and
    // active on launch. Library (manage/install backends) sits directly below.
    nav.insert()
        .text("Models")
        .data::<crate::state::Page>(crate::state::Page::Models)
        .icon(icons::phosphor(icons::BRAIN))
        .activate();

    nav.insert()
        .text("Library")
        .data::<crate::state::Page>(crate::state::Page::Library)
        .icon(icons::phosphor(icons::BOOKS));

    nav.insert()
        .text("Customization")
        .data::<crate::state::Page>(crate::state::Page::Customization)
        .icon(icons::phosphor(icons::GEAR));

    nav.insert()
        .text("Recording")
        .data::<crate::state::Page>(crate::state::Page::Recording)
        .icon(icons::phosphor(icons::MICROPHONE));

    nav.insert()
        .text("Input Simulation")
        .data::<crate::state::Page>(crate::state::Page::InputSimulation)
        .icon(icons::phosphor(icons::KEYBOARD));

    nav.insert()
        .text("Connection")
        .data::<crate::state::Page>(crate::state::Page::Connection)
        .icon(icons::phosphor(icons::PLUG));

    nav
}

/// Builds the Models-page tab bar with Installed and Browse tabs.
fn build_models_tabs() -> segmented_button::SingleSelectModel {
    let mut models_tabs = segmented_button::SingleSelectModel::default();
    models_tabs
        .insert()
        .text("Installed")
        .data(ModelsTab::Installed)
        .activate();
    models_tabs
        .insert()
        .text("Browse")
        .data(ModelsTab::Download);
    models_tabs
}

/// Builds the initial batch of startup tasks (audio themes, daemon ping, data load).
fn initial_load_tasks(
    title_command: Task<cosmic::Action<Message>>,
) -> Task<cosmic::Action<Message>> {
    // Load audio themes on startup (always available)
    let load_themes = Task::perform(load_audio_themes(), |themes| {
        cosmic::Action::App(Message::Recording(RecordingMessage::AudioThemesLoaded(
            themes,
        )))
    });

    // Try to ping the daemon on startup
    let initial_ping = Task::perform(ping_daemon(), |result| {
        cosmic::Action::App(match result {
            Ok(_) => Message::Daemon(DaemonMessage::DaemonConnected),
            Err(e) => Message::Daemon(DaemonMessage::DaemonError(e)),
        })
    });

    // Load initial data (models + device info) on startup
    let load_initial_data = Task::perform(
        async move {
            // Small delay to let daemon connection establish
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        },
        |()| cosmic::Action::App(Message::Model(ModelMessage::LoadInitialData)),
    );

    Task::batch([title_command, load_themes, initial_ping, load_initial_data])
}

impl AppModel {
    /// Initializes the application with any given flags and startup commands.
    pub(super) fn init_model(
        core: cosmic::Core,
        _flags: (),
    ) -> (Self, Task<cosmic::Action<Message>>) {
        let nav = build_nav();
        let models_tabs = build_models_tabs();

        // Construct the app model with the runtime's core.
        let mut app = AppModel {
            core,
            context_page: ContextPage::default(),
            nav,
            // Initialize Super STT state using proper socket path
            socket_path: super_stt_shared::validation::get_http_socket_path(),
            daemon_status: DaemonStatus::Disconnected,
            reconnect_retry: super_stt_shared::daemon::retry::RetryStrategy::for_initial_connection(
            ),
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
            available_models: Vec::new(),
            current_model: String::new(),
            current_provider: Provider::default(),
            current_source: String::new(),
            current_model_epoch: 0,
            model_operation_state: ModelOperationState::Loading {
                target_model: String::new(),
                status_message: "Loading initial model state...".to_string(),
            },

            // Initialize device state
            current_device: String::new(), // Empty until loaded from daemon
            available_devices: vec!["cpu".to_string()], // Default until loaded from daemon
            gpu_info: Vec::new(),
            device_state: DeviceState::Ready,
            last_switch_progress_at: None,
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

            // Models page UI state
            models_tabs,
            active_backend: None,
            staged_model: None,
            staged_device: None,
            configure_backend: None,
            installed_menu_open: None,

            // Transcription language state
            primary_language: None,
            model_language: None,
            model_language_for: None,
            language_picker_target: None,
            language_picker_query: String::new(),

            // Backend catalog + per-backend configuration state
            backends: Vec::new(),
            backend_secret_inputs: HashMap::new(),
            backend_secret_configured: HashMap::new(),
            backend_option_inputs: HashMap::new(),

            // Registry state
            registry: crate::state::registry::RegistryState::default(),

            // No pending scoped action error at startup.
            action_error: None,
        };

        // Create startup commands
        let title_command = app.update_title();
        (app, initial_load_tasks(title_command))
    }
}
