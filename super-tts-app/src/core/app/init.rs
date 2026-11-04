// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::client::load_audio_themes;
use crate::state::{AudioTheme, ContextPage, DaemonStatus, SpeakingStatus};
use crate::ui::icons;
use crate::ui::messages::{Message, ModelMessage, SpeechMessage};
use cosmic::prelude::*;
use cosmic::widget::nav_bar;
use std::collections::HashMap;

use super::{AppModel, DeviceState, ModelOperationState};

/// Builds the navigation bar with all Super TTS pages inserted in order.
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
        .text("Speech")
        .data::<crate::state::Page>(crate::state::Page::Speech)
        .icon(icons::phosphor(icons::PLAY));

    nav.insert()
        .text("Voices")
        .data::<crate::state::Page>(crate::state::Page::Voices)
        .icon(icons::phosphor(icons::MICROPHONE));

    nav.insert()
        .text("Connection")
        .data::<crate::state::Page>(crate::state::Page::Connection)
        .icon(icons::phosphor(icons::PLUG));

    nav.insert()
        .text("Updates")
        .data::<crate::state::Page>(crate::state::Page::Updates)
        .icon(icons::phosphor(icons::ARROWS_CLOCKWISE));

    nav
}

/// Builds the initial batch of startup tasks (audio themes, daemon ping, data load).
fn initial_load_tasks(
    title_command: Task<cosmic::Action<Message>>,
) -> Task<cosmic::Action<Message>> {
    // Load audio themes on startup (always available)
    let load_themes = Task::perform(load_audio_themes(), |themes| {
        cosmic::Action::App(Message::Speech(SpeechMessage::AudioThemesLoaded(themes)))
    });

    // Try to ping the daemon on startup
    let initial_ping = crate::core::app::handlers::tasks::ping_task();

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

        // Construct the app model with the runtime's core.
        let mut app = AppModel {
            core,
            context_page: ContextPage::default(),
            nav,
            // Initialize Super TTS state using proper socket path
            socket_path: super_tts_shared::validation::get_http_socket_path(),
            daemon_status: DaemonStatus::Disconnected,
            reconnect_retry: super_tts_shared::daemon::retry::RetryStrategy::for_initial_connection(
            ),
            speaking_status: SpeakingStatus::Idle,
            speaking_utterance: None,
            speech_test_text: String::new(),
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
            current_source: String::new(),
            current_model_epoch: 0,
            model_operation_state: ModelOperationState::Loading {
                target_model: String::new(),
                status_message: "Loading initial model state...".to_string(),
                load: None,
            },

            // Device state. Every one of these starts empty rather than
            // guessing a `["cpu"]` the daemon has not confirmed: which devices
            // exist is a per-model question now, answered per model, and a
            // seeded default would be indistinguishable from a real answer for
            // a model that can in fact run nowhere on this host.
            current_device: String::new(),
            stage_devices: Vec::new(),
            model_device: None,
            model_device_for: None,
            gpu_info: Vec::new(),
            device_state: DeviceState::Ready,
            last_switch_progress_at: None,
            last_event_timestamp: None,

            notification_method:
                super_tts_shared::models::notification_method::NotificationMethod::default(),
            volume: 100,
            last_committed_volume: 100,

            // Custom models directory
            custom_models_dir: None,
            custom_models_dir_input: String::new(),

            // Models page UI state
            models_page: crate::state::models_page::ModelsPageState::default(),

            // Speech language state
            language: crate::state::language::LanguageState::default(),
            voice: crate::state::VoiceState::default(),

            // Backend catalog + per-backend configuration state
            backends: Vec::new(),
            stage_backends: Vec::new(),
            backend_secret_inputs: HashMap::new(),
            backend_secret_configured: HashMap::new(),
            backend_option_inputs: HashMap::new(),

            // Registry state
            registry: crate::state::registry::RegistryState::default(),

            // Self-update state
            update: crate::state::update::UpdateState::default(),

            // Cloned-voice library; read on connect, not at construction.
            voices: crate::state::voices::VoicesState::default(),

            // No pending scoped action error at startup.
            action_error: None,
        };

        // Create startup commands
        let title_command = app.update_title();
        (app, initial_load_tasks(title_command))
    }
}
