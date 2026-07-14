// SPDX-License-Identifier: GPL-3.0-only
mod init;
pub mod messages;
mod subscription;
mod update;
mod view;

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use cosmic::{
    Element, app as cosmic_app,
    iced::{Subscription, window},
    widget::segmented_button::{Entity, SingleSelectModel},
};

pub use messages::*;

use crate::config::AppletConfig;
use crate::daemon::RetryStrategy;
use crate::models::state::{DaemonConnectionState, IsOpen, RecordingState};
use crate::models::theme::{ThemeConfig, VisualizationSide};
use crate::ui::components::sound_visualization::VisualizationComponent;
use crate::ui::components::working_animation_component::WorkingAnimationComponent;
use subscription::{PING_INTERVAL_SECS, UdpSubscriptionId, applet_events_subscription};

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
    working_animation: WorkingAnimationComponent,
    /// Wall-clock start of the current Processing phase; `Some` only while
    /// transcribing, used to derive the animation's elapsed time.
    working_anim_start: Option<Instant>,
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
        Self::new(core, visualization_side)
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
        let mut subs = vec![
            // Daemon `/events` SSE subscription, keyed on
            // `udp_restart_counter` so a forced re-auth
            // (`Message::RetryAuthorization`) tears down the old stream
            // and starts a fresh one with whatever auth state the daemon
            // now has. The `UdpSubscriptionId` name is retained while the
            // legacy UDP path is being deprecated.
            Subscription::run_with(
                UdpSubscriptionId(self.udp_restart_counter),
                applet_events_subscription,
            ),
            cosmic::iced::time::every(Duration::from_secs(PING_INTERVAL_SECS))
                .map(|_| Message::PingTimeout),
        ];
        if self.daemon_state == DaemonConnectionState::Connected
            && matches!(self.recording_state, RecordingState::Processing)
        {
            subs.push(
                cosmic::iced::time::every(Duration::from_millis(33))
                    .map(|_| Message::WorkingAnimationTick),
            );
        }
        Subscription::batch(subs)
    }

    fn update(&mut self, message: Self::Message) -> cosmic_app::Task<Self::Message> {
        self.handle_message(message)
    }

    fn view(&self) -> Element<'_, Message> {
        self.view_applet()
    }

    fn view_window(&self, _id: window::Id) -> Element<'_, Message> {
        self.view_popup()
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Message> {
        Some(Message::CloseRequested(id))
    }
}
