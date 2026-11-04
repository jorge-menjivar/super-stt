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
use crate::daemon::client::v1::pipeline::device::ModelDevice;
use crate::state::{AudioTheme, ContextPage, DaemonStatus, MenuAction, SpeakingStatus};
use crate::ui::messages::{DaemonMessage, Message, ModelsPageMessage, ShellMessage};
use cosmic::app::context_drawer;
use cosmic::iced::Subscription;
use cosmic::prelude::*;
use cosmic::widget::{menu, nav_bar};
use std::collections::HashMap;
use std::path::PathBuf;

/// Unified model operation state that encompasses downloading, loading, and switching
#[derive(Debug, Clone)]
pub enum ModelOperationState {
    /// Model is ready for use
    Ready,
    /// Provisioning the model's files: verifying the copies already on disk,
    /// and downloading the ones that are missing or stale. One state for both
    /// phases because the daemon reports them through one byte-tracked
    /// progress shape and the UI treats them alike; `progress.status`
    /// (`"verifying"` / `"downloading"`) says which is running, and is the only
    /// thing the card's wording depends on.
    Provisioning {
        target_model: String,
        progress: super_tts_shared::models::protocol::DownloadProgress,
    },
    /// Loading model into memory (after the files are all on disk)
    Loading {
        target_model: String,
        status_message: String,
        /// What the backend reports of its load — a first-time setup, the
        /// step it is on, how far through — once it reports anything.
        load: Option<super_tts_shared::models::protocol::LoadProgress>,
    },
    /// Model operation failed
    Error { message: String },
}

/// Whether a device change is in flight for the model the stage is running.
///
/// Only a change to the *loaded* model's device reloads anything; a change to
/// any other model is a note for its next load and passes through this state
/// untouched. The daemon narrates the reload over `/events`
/// (`switching_device`, then `loading_model_for_device`, then `ready`), and
/// this is what lets those arrive in any order without the app deciding a
/// switch finished before it started.
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceState {
    Ready,
    Switching {
        target_device: String,
        status_message: String,
    },
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

    // Super TTS specific state
    /// Socket path for daemon communication
    pub socket_path: PathBuf,
    /// Current daemon connection status
    pub daemon_status: DaemonStatus,
    /// Reconnect backoff (shared `RetryStrategy`: exponential + jitter),
    /// advanced on each failed reconnect and reset on a successful connection.
    pub reconnect_retry: super_tts_shared::daemon::retry::RetryStrategy,
    /// Whether the daemon is speaking right now.
    pub speaking_status: SpeakingStatus,
    /// The utterance the daemon is playing, when it named one. Not necessarily
    /// this app's: any client can ask the daemon to speak, and the badge
    /// reflects the device, not this page.
    pub speaking_utterance: Option<String>,
    /// Text in the Speech page's test field.
    pub speech_test_text: String,
    /// Current audio level (0.0 to 1.0)
    pub audio_level: f32,
    /// Whether audio is currently coming out of the speakers.
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
    /// Available models from daemon as `(name, source)` tuples.
    pub available_models: Vec<(String, String)>,
    /// Currently loaded model
    pub current_model: String,
    /// Source (serving backend repo id) of the currently loaded model
    pub current_source: String,
    /// Monotonic counter bumped every time a live daemon event (e.g.
    /// `model_switched`) updates the model identity. A point-in-time stage-view
    /// snapshot captures this at issue time and is discarded on arrival if the
    /// counter has since advanced — so a slow, stale reconnect query can never
    /// clobber a fresher live event.
    pub current_model_epoch: u64,
    /// Model operation state (downloading, loading, or ready)
    pub model_operation_state: ModelOperationState,

    // Device management state.
    //
    // The device preference belongs to a *model*, not to the daemon. A small
    // preset-voice model runs fine on the CPU while the cloning model beside it
    // needs the GPU, and the single global setting this replaced meant that
    // choosing for one silently overwrote the choice for the other. So the
    // answer is held per model and guarded by the pair it describes; the
    // stage-wide list is only what can be said before a model is picked.
    /// The accelerator the loaded model is actually running on (`cuda`, `rocm`,
    /// `cpu`, …); empty when nothing is loaded.
    ///
    /// Deliberately not a preference. A `gpu` choice that fell back to the CPU
    /// reads `cpu` here, which is the point: the card's "Active:" line would
    /// otherwise tell the user they are on a GPU they are not on. It is filled
    /// from the stage view's resolved accelerator and from the daemon's `ready`
    /// event — never from what the user staged.
    pub current_device: String,
    /// The devices the stage's selected backend can run models on here, from
    /// `GET /pipeline/{stage}/device/list`.
    ///
    /// The union over the models that backend serves, and so the only device
    /// answer available before a model is picked. It keeps the card's device
    /// control populated in the beat between staging a model and that model's
    /// own, narrower list arriving, rather than letting the control appear a
    /// moment after the model dropdown it sits beside. Empty until a backend is
    /// selected.
    pub stage_devices: Vec<String>,
    /// One model's device answer: the preference it will load with, what that
    /// resolved to, and the devices this host can offer it.
    ///
    /// A single slot rather than a map, mirroring `language.model_language`:
    /// the card only ever asks about the model it is showing, and a map would
    /// keep answers for models whose backend has since been swapped out — the
    /// stalest possible thing to render a device picker from.
    pub model_device: Option<ModelDevice>,
    /// Which `(source, model)` pair [`Self::model_device`] describes. Reading
    /// the block without checking this is how a card ends up showing one
    /// model's device list under another model's name.
    pub model_device_for: Option<(String, String)>,
    /// GPU inventory + memory from the daemon's `GET /gpu_info` (gpu-probe).
    pub gpu_info: Vec<super_tts_shared::models::protocol::GpuInfo>,
    /// Device switching state
    pub device_state: DeviceState,
    /// Timestamp of the last model-switch progress signal (switch start or any
    /// `download_progress` tick). Drives the stall watchdog in the
    /// `PingTimeout` handler so a switch that stops making progress surfaces an
    /// error instead of spinning forever. `None` when no switch is in flight.
    pub last_switch_progress_at: Option<std::time::Instant>,
    /// Last event timestamp for polling daemon events
    pub last_event_timestamp: Option<String>,

    // Notification method
    pub notification_method: super_tts_shared::models::notification_method::NotificationMethod,

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

    // Speech language state (global Primary Language + per-model picker).
    pub language: crate::state::language::LanguageState,

    // The active model's voice: which one it speaks in, and which it could.
    pub voice: crate::state::VoiceState,

    // Installed-backend catalog and per-backend configuration state.
    /// Backends discovered by the daemon, with the models/secrets/options
    /// each declares. Drives the per-backend sections on the Models page.
    pub backends: Vec<BackendInfo>,
    /// The subset of [`Self::backends`] that can fill the synthesis stage, as
    /// `GET /pipeline/{stage}/backend/list` reports it.
    ///
    /// What the "Select a backend" sheet may offer. Selecting a backend that
    /// serves nothing this stage can run is refused by the daemon, so a sheet
    /// built from the full catalog offers picks that fail on click — and while
    /// synthesis is the only stage the two lists are identical, which is
    /// exactly what would let the wrong one stand in unnoticed.
    pub stage_backends: Vec<BackendInfo>,
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

    // The cloned-voice library, plus the recording or import in progress.
    pub voices: crate::state::voices::VoicesState,

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

    /// The daemon's device answer for one model, iff the held block is that
    /// model's. The pair check is the whole value of this accessor: staging a
    /// second model leaves the previous model's block in place until its own
    /// answer lands, and a device picker drawn from it would be showing the
    /// wrong model's options under the right model's name.
    fn model_device_answer(&self, source: &str, model: &str) -> Option<&ModelDevice> {
        let (held_source, held_model) = self.model_device_for.as_ref()?;
        if held_source != source || held_model != model {
            return None;
        }
        self.model_device.as_ref()
    }

    /// The devices this host can offer one model, as the daemon narrowed them.
    ///
    /// `None` is not "none offered" — it is the window between staging a model
    /// and its answer arriving. A caller that collapsed the two would announce
    /// "this model can't be loaded here" for a moment on every single pick,
    /// which is why the empty list and the missing answer stay distinguishable
    /// all the way up into the view.
    #[must_use]
    pub fn model_devices(&self, source: &str, model: &str) -> Option<&[String]> {
        self.model_device_answer(source, model)
            .map(|d| d.available_devices.as_slice())
    }

    /// The device preference the daemon has recorded for one model — what it
    /// would load with today, `cpu`/`gpu`, or `none` for a model that
    /// synthesizes remotely. `None` until the answer for this pair arrives.
    ///
    /// This is what a Load click compares its staged device against: writing a
    /// preference the daemon already holds costs a request and, for the model a
    /// stage is running, a reload of a model that is already where it belongs.
    #[must_use]
    pub fn model_device_preference(&self, source: &str, model: &str) -> Option<&str> {
        self.model_device_answer(source, model)
            .map(|d| d.device.as_str())
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
    const APP_ID: &'static str = "ai.menjivar.super-tts-app";

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
                .push(cosmic::widget::text("Super TTS").size(16.0)),
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

        // The recording meter's refresh rate. Fast enough that a voice reads
        // as live movement rather than a stepping bar, slow enough to cost
        // nothing — and present only while the microphone is open, so an idle
        // app is not waking ten times a second to redraw nothing.
        const RECORDING_TICK_MS: u64 = 100;

        let mut subscriptions = vec![
            // HTTP /events SSE subscription. Covers the playback /
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
        ];
        if self.voices.is_recording() {
            subscriptions.push(
                cosmic::iced::time::every(std::time::Duration::from_millis(RECORDING_TICK_MS))
                    .map(|_| Message::Voices(crate::ui::messages::VoicesMessage::RecordingTick)),
            );
        }
        // The Add-a-backend drawer's spinner is a still SVG per angle, so it
        // needs a tick to step through them. Only while a check is actually in
        // flight: outside that the drawer draws no spinner, and a timer left
        // running would wake the app for nothing.
        if self.registry.add_preview.is_checking() {
            subscriptions.push(
                cosmic::iced::time::every(crate::ui::icons::SPINNER_FRAME)
                    .map(|_| Message::ModelsPage(ModelsPageMessage::AddPreviewSpin)),
            );
        }
        Subscription::batch(subscriptions)
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
        self.models_page.browse_menu_open = false;

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
            // The library and the loaded model's cloning capability are both
            // read here rather than held from connect-time: a model switch
            // since then changes what the page says it can do, and another
            // client may have added or removed a voice.
            Some(crate::state::Page::Voices) => Task::batch([
                self.dispatch(Message::Voices(crate::ui::messages::VoicesMessage::Refresh)),
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
