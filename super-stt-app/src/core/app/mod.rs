// SPDX-License-Identifier: GPL-3.0-only

mod events;
mod handlers;
mod init;
mod small_state;
mod subscription;
mod update;
mod view;
use subscription::{UdpSubscriptionId, audio_events_subscription};

use crate::daemon::backends::BackendInfo;
use crate::state::{AudioTheme, ContextPage, DaemonStatus, MenuAction, RecordingStatus};
use crate::ui::messages::Message;
use cosmic::app::context_drawer;
use cosmic::iced::Subscription;
use cosmic::prelude::*;
use cosmic::widget::{menu, nav_bar, segmented_button};
use std::collections::HashMap;
use std::path::PathBuf;
use super_stt_shared::models::provider::Provider;

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
// reason: AppModel mirrors discrete UI toggles; COSMIC apps accumulate independent bool flags.
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
    /// Reconnect backoff (shared `RetryStrategy`: exponential + jitter),
    /// advanced on each failed reconnect and reset on a successful connection.
    pub reconnect_retry: super_stt_shared::daemon::retry::RetryStrategy,
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
    /// Available models from daemon as `(name, provider, source)` tuples.
    pub available_models: Vec<(String, Provider, String)>,
    /// Currently loaded model
    pub current_model: String,
    /// Provider of the currently loaded model
    pub current_provider: Provider,
    /// Source (serving backend repo id) of the currently loaded model
    pub current_source: String,
    /// Monotonic counter bumped every time a live daemon event (e.g.
    /// `model_switched`) updates the model identity. A point-in-time
    /// `get_current_model` snapshot captures this at issue time and is
    /// discarded on arrival if the counter has since advanced — so a slow,
    /// stale reconnect query can never clobber a fresher live event.
    pub current_model_epoch: u64,
    /// Model operation state (downloading, loading, or ready)
    pub model_operation_state: ModelOperationState,

    // Device management state
    /// Current device (cpu/cuda) from daemon
    pub current_device: String,
    /// Available devices from daemon
    pub available_devices: Vec<String>,
    /// GPU inventory + memory from the daemon's `GET /gpu_info` (gpu-probe).
    pub gpu_info: Vec<super_stt_shared::models::protocol::GpuInfo>,
    /// Device switching state
    pub device_state: DeviceState,
    /// Timestamp of the last model-switch progress signal (switch start or any
    /// `download_progress` tick). Drives the stall watchdog in the
    /// `PingTimeout` handler so a switch that stops making progress surfaces an
    /// error instead of spinning forever. `None` when no switch is in flight.
    pub last_switch_progress_at: Option<std::time::Instant>,
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

    // Models page UI state
    /// Installed / Download tab bar for the Models page (active tab carries a
    /// [`ModelsTab`] as its data).
    pub models_tabs: segmented_button::SingleSelectModel,
    /// Source of the currently-selected (active) backend, shown in the card
    /// above the tabs. `None` when the daemon is idle.
    pub active_backend: Option<String>,
    /// Model the user has picked in the active-backend card's dropdown but
    /// hasn't yet committed via the Load button. Cleared once a model is
    /// loaded or the user changes backends.
    pub staged_model: Option<String>,
    /// Device the user has picked for the staged model (CPU / CUDA / Metal,
    /// or `"none"` for an online model that needs no device choice). Cleared
    /// alongside [`Self::staged_model`].
    pub staged_device: Option<String>,
    /// The backend whose configuration sub-view is open, if any (`source`).
    pub configure_backend: Option<String>,

    // Transcription language state.
    /// Global Primary Language from the daemon (None = unset). Display-only cache.
    pub primary_language: Option<String>,
    /// Resolution block from `GET /backends/{source}/models/{model}/language`
    /// for the model identified by `model_language_for`.
    pub model_language: Option<serde_json::Value>,
    /// Which `(source, model)` pair `model_language` belongs to. Guards
    /// stale-block display: only use `model_language` when this matches
    /// the target `(source, model)`.
    pub model_language_for: Option<(String, String)>,
    /// The `(source, model)` pair the open per-model language sheet configures.
    /// `None` when the sheet is in global mode.
    pub language_picker_target: Option<(String, String)>,
    /// Live query text for the language search sheet.
    pub language_picker_query: String,
    /// `source` of the installed-backend card whose overflow ("⋯") menu is
    /// open, if any. Only one is open at a time.
    pub installed_menu_open: Option<String>,

    // Installed-backend catalog and per-backend configuration state.
    /// Backends discovered by the daemon, with the models/secrets/options
    /// each declares. Drives the per-backend sections on the Models page.
    pub backends: Vec<BackendInfo>,
    /// In-progress text for each secret input, keyed by `(source, name)`.
    pub backend_secret_inputs: HashMap<(String, String), String>,
    /// Whether each declared secret is currently configured, as reported by
    /// the daemon, keyed by `(source, name)`. Refreshed on catalog load.
    pub backend_secret_configured: HashMap<(String, String), bool>,
    /// In-progress text for each option input, keyed by `(source, name)`.
    pub backend_option_inputs: HashMap<(String, String), String>,

    // Registry state (catalog + install progress for the Download tab).
    pub registry: crate::state::registry::RegistryState,

    /// Scope-tagged banner for a failed settings/backend save. Rendered inline
    /// on the owning page instead of hijacking the UI (Tier 1 #13) or being
    /// dropped to the log (Tier 1 #15). `None` when there is no pending error.
    pub action_error: Option<crate::state::ActionError>,
}

impl AppModel {
    /// The pending [`action_error`](Self::action_error) message iff it is tagged
    /// for `scope` — used by each page's view to render its own inline banner.
    #[must_use]
    pub fn action_error_for(&self, scope: crate::state::ErrorScope) -> Option<&str> {
        self.action_error
            .as_ref()
            .filter(|e| e.scope == scope)
            .map(|e| e.message.as_str())
    }
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
    fn init(core: cosmic::Core, flags: Self::Flags) -> (Self, Task<cosmic::Action<Self::Message>>) {
        Self::init_model(core, flags)
    }

    /// Elements to pack at the start of the header bar.
    fn header_start(&self) -> Vec<Element<'_, Self::Message>> {
        // App name + logo, pinned to the header's leading edge so the window
        // always identifies itself. The logo tints to the accent; the name sits
        // beside it in the header's text color.
        let accent: cosmic::iced::Color = cosmic::theme::active().cosmic().accent.base.into();
        let brand = cosmic::widget::container(
            cosmic::widget::row::with_capacity(2)
                .align_y(cosmic::iced::Alignment::Center)
                .spacing(8.0)
                .push(crate::ui::icons::app_logo(22.0, accent))
                .push(cosmic::widget::text("Super STT").size(16.0)),
        )
        .padding([0, 12, 0, 8]);

        let menu_bar = menu::bar(vec![menu::Tree::with_children(
            menu::root("View").apply(Element::from),
            menu::items(
                &HashMap::new(),
                vec![menu::Item::Button("About", None, MenuAction::About)],
            ),
        )]);

        vec![brand.into(), menu_bar.into()]
    }

    /// Window header-bar readouts (right side): the GPU summary and model
    /// readiness pills.
    fn header_end(&self) -> Vec<Element<'_, Self::Message>> {
        self.header_end_impl()
    }

    /// Enables the COSMIC application to create a nav bar with this model.
    fn nav_model(&self) -> Option<&nav_bar::Model> {
        self.nav_model_impl()
    }

    /// Display a context drawer if the context page is requested.
    fn context_drawer(&self) -> Option<context_drawer::ContextDrawer<'_, Self::Message>> {
        self.context_drawer_impl()
    }

    /// Describes the interface based on the current state of the application model.
    ///
    /// Application events will be processed through the view. Any messages emitted by
    /// events received by widgets will be passed to the update method.
    fn view(&self) -> Element<'_, Self::Message> {
        self.view_impl()
    }

    /// Register subscriptions for this application.
    ///
    /// Subscriptions are long-running async tasks running in the background which
    /// emit messages to the application through a channel. They are started at the
    /// beginning of the application, and persist through its lifetime.
    fn subscription(&self) -> Subscription<Self::Message> {
        // Connection monitoring constants
        const PING_INTERVAL_SECS: u64 = 5;
        // GPU memory changes as models load/unload and as other processes use
        // the card, so poll it a bit faster than the ping to keep the header
        // readout (and the staged-load fit warning) live.
        const GPU_POLL_INTERVAL_SECS: u64 = 3;

        Subscription::batch(vec![
            // HTTP /events SSE subscription. Covers the recording /
            // audio-meter topics and the model/device/download status
            // topics — the settings app's token holds every scope these
            // need, so one subscription carries the full set.
            Subscription::run_with(
                UdpSubscriptionId(self.udp_restart_counter),
                audio_events_subscription,
            ),
            // Periodic connection monitoring
            cosmic::iced::time::every(std::time::Duration::from_secs(PING_INTERVAL_SECS))
                .map(|_| Message::PingTimeout),
            // Periodic GPU inventory/memory refresh (gated on connection in the
            // handler, so it's a no-op while disconnected).
            cosmic::iced::time::every(std::time::Duration::from_secs(GPU_POLL_INTERVAL_SECS))
                .map(|_| Message::RefreshGpuInfo),
        ])
    }

    /// Handles messages emitted by the application and its widgets.
    ///
    /// Tasks may be returned for asynchronous execution of code in the background
    /// on the application's async runtime.
    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        self.dispatch(message)
    }

    /// Called when a nav item is selected.
    fn on_nav_select(&mut self, id: nav_bar::Id) -> Task<cosmic::Action<Self::Message>> {
        // Activate the page in the model.
        self.nav.activate(id);
        // Leaving the current section dismisses any open context sheet
        // (e.g. the Models page's "Add backend" / configuration drawer) and
        // any open per-card overflow menu.
        self.core.window.show_context = false;
        self.installed_menu_open = None;

        self.update_title()
    }
}

impl menu::action::MenuAction for MenuAction {
    type Message = Message;

    fn message(&self) -> Self::Message {
        match self {
            MenuAction::About => Message::ToggleContextPage(ContextPage::About),
        }
    }
}
