// SPDX-License-Identifier: GPL-3.0-only

mod events;
mod handlers;
mod init;
mod small_state;
mod subscription;
mod update;
pub(crate) mod updater;
mod view;
use subscription::{UdpSubscriptionId, audio_events_subscription};

use crate::daemon::backends::BackendInfo;
use crate::state::{AudioTheme, ContextPage, DaemonStatus, MenuAction, RecordingStatus};
use crate::ui::messages::{DaemonMessage, Message, ModelsPageMessage, ShellMessage};
use cosmic::app::context_drawer;
use cosmic::iced::Subscription;
use cosmic::prelude::*;
use cosmic::widget::{menu, nav_bar};
use std::collections::HashMap;
use std::path::PathBuf;

/// The model operation a pipeline stage has in flight. Defined with the
/// per-stage container that holds it, and re-exported here because every
/// handler reaches it through the app model.
pub use crate::state::model_operations::{ModelOperationState, ModelOperations};

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
    /// Latest incremental preview, shown on its own line above the final text.
    /// Previews and finals used to share `transcription_text`, so a preview was
    /// indistinguishable from a result; keeping them apart is what makes a
    /// realtime model's incremental output visible as such.
    pub preview_text: String,
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
    /// Currently loaded model
    pub current_model: String,
    /// Source (serving backend repo id) of the currently loaded model
    pub current_source: String,
    /// Monotonic counter bumped every time a live daemon event (e.g.
    /// `model_switched`) updates the model identity. A point-in-time
    /// `get_current_model` snapshot captures this at issue time and is
    /// discarded on arrival if the counter has since advanced — so a slow,
    /// stale reconnect query can never clobber a fresher live event.
    pub current_model_epoch: u64,
    /// What each pipeline stage has in flight: a download, a load, a failure,
    /// or nothing. Per stage because the stages provision independently, so
    /// one stage's work must neither render on another's card nor keep its
    /// Load button disabled.
    pub model_operations: ModelOperations,

    // Device management state
    /// The accelerator the loaded stage-1 model is on (`cpu`/`cuda`/…), from
    /// the daemon's `ready` events; empty when nothing is loaded. Each
    /// model's own cpu/gpu *preference* is read from the daemon when the
    /// model is staged.
    pub current_device: String,
    /// GPU inventory + memory from the daemon's `GET /gpu_info` (gpu-probe).
    pub gpu_info: Vec<super_stt_shared::models::protocol::GpuInfo>,
    /// Device switching state
    pub device_state: DeviceState,
    /// Last event timestamp for polling daemon events
    pub last_event_timestamp: Option<String>,

    // Preview typing state
    /// Whether preview typing is enabled (beta feature)
    pub preview_typing_enabled: bool,

    /// The transcript post-processor: whether it runs, which model is
    /// selected, and whether that model is actually loaded. Mirrors the
    /// daemon's `/post_processor` block; the daemon stays authoritative, so
    /// this is replaced wholesale by every `Loaded` message rather than being
    /// edited field-by-field.
    pub post_processor: crate::daemon::client::StageState,
    /// What each stage's card has picked but not yet loaded. One store for
    /// every stage: a pick is local until Load commits it, which is true of
    /// every stage's card, so there is no per-stage copy of the rule.
    pub staged_picks: crate::state::staged_picks::StagedPicks,
    /// The device lists the daemon has answered for, per pipeline stage: what
    /// its selected backend can run models on, and what each staged model can
    /// be loaded onto. Read by the pickers instead of deriving availability
    /// from the catalog, so the app and the daemon cannot disagree about what
    /// this install can run.
    pub device_offers: crate::state::device_offers::DeviceOffers,
    /// What the daemon says each stage can be filled with — its eligible
    /// backends and their models for that stage. Read rather than derived from
    /// the catalog, so a picker cannot offer what the daemon would refuse.
    pub stage_catalog: crate::state::stage_catalog::StageCatalog,

    // Recording stop mode
    pub recording_stop_mode: super_stt_shared::models::recording_stop_mode::RecordingStopMode,

    // Write method
    pub write_method: super_stt_shared::models::write_method::WriteMethod,
    /// Text in the Input Simulation test field — where `POST /write_method/test`
    /// lands, since the daemon types into whatever window has focus.
    pub write_method_test_text: String,
    /// Backend the last test actually typed through. `None` until a test runs;
    /// with `write_method == Auto` this is the only readout of the real backend.
    pub resolved_write_method: Option<super_stt_shared::models::write_method::WriteMethod>,
    /// Seconds left before a delayed write-method test types. `None` when no
    /// countdown is running, which is also how a cancel is recorded.
    pub write_method_test_countdown: Option<u8>,

    // Notification method
    pub notification_method: super_stt_shared::models::notification_method::NotificationMethod,

    // Master volume (0-100)
    pub volume: u8,
    /// Last value successfully committed to (or loaded from) the daemon. The
    /// rollback target when a `VolumeCommit` save fails, since the live drag has
    /// already overwritten `volume` (audit Tier 3 #37).
    pub last_committed_volume: u8,

    // Custom models directory
    pub custom_models_dir: Option<String>,
    pub custom_models_dir_input: String,

    // Models page UI state (tabs, active-backend card selection/staging, menus).
    pub models_page: crate::state::models_page::ModelsPageState,

    // Transcription language state (global Primary Language + per-model picker).
    pub language: crate::state::language::LanguageState,

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

    // Self-update state (Updates page + header badge + apply flow).
    pub update: crate::state::update::UpdateState,

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

    /// Park a failed action in the single scope-tagged banner slot. The one
    /// slot is deliberate — only one page is visible at a time, and
    /// [`action_error_for`](Self::action_error_for) gates rendering by scope.
    pub fn set_action_error(&mut self, scope: crate::state::ErrorScope, message: String) {
        self.action_error = Some(crate::state::ActionError { scope, message });
    }

    /// Clear the banner only if it currently belongs to `scope`, so retrying or
    /// succeeding at one page's action can't wipe another page's pending error.
    pub fn clear_action_error(&mut self, scope: crate::state::ErrorScope) {
        if self.action_error.as_ref().is_some_and(|e| e.scope == scope) {
            self.action_error = None;
        }
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
        // always identifies itself. The logo keeps its own brand colors; the
        // name sits beside it in the header's text color.
        let brand = cosmic::widget::container(
            cosmic::widget::row::with_capacity(2)
                .align_y(cosmic::iced::Alignment::Center)
                .spacing(8.0)
                .push(crate::ui::icons::app_logo(22.0))
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
                .map(|_| Message::Daemon(DaemonMessage::PingTimeout)),
            // Periodic GPU inventory/memory refresh (gated on connection in the
            // handler, so it's a no-op while disconnected).
            cosmic::iced::time::every(std::time::Duration::from_secs(GPU_POLL_INTERVAL_SECS))
                .map(|_| Message::ModelsPage(ModelsPageMessage::RefreshGpuInfo)),
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
        self.models_page.installed_menu_open = None;

        // Opening a page that draws backend cards refetches both catalogs. The
        // daemon reads `installed_version` off disk on every request, so it is
        // never the stale one — the app was, holding a catalog fetched once at
        // startup. Anything that changed a backend outside this app (an install
        // from the CLI, an edited `backend.toml`) went unseen until a restart.
        match self.nav.data::<crate::state::Page>(id) {
            Some(crate::state::Page::Models | crate::state::Page::Library) => Task::batch([
                handlers::tasks::reload_backend_catalogs(),
                self.update_title(),
            ]),
            // Re-fetch rather than trust whatever was loaded at connect-time —
            // a periodic background check may have completed since, or another
            // client may have changed the beta-opt-in setting.
            Some(crate::state::Page::Updates) => Task::batch([
                handlers::tasks::refresh_update_status(),
                self.update_title(),
            ]),
            _ => self.update_title(),
        }
    }
}

impl menu::action::MenuAction for MenuAction {
    type Message = Message;

    fn message(&self) -> Self::Message {
        match self {
            MenuAction::About => {
                Message::Shell(ShellMessage::ToggleContextPage(ContextPage::About))
            }
        }
    }
}
