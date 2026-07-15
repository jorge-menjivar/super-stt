// SPDX-License-Identifier: GPL-3.0-only
use cosmic::{
    app as cosmic_app,
    widget::segmented_button::{Entity, SingleSelectModel},
};
use log::info;

use super::SuperSttApplet;
use crate::app::Message;
use crate::config::AppletConfig;
use crate::daemon::{RetryStrategy, ping_daemon};
use crate::models::state::{DaemonConnectionState, IsOpen, RecordingState};
use crate::models::theme::{IconAlignment, VisualizationSide};
use crate::ui::components::sound_visualization::VisualizationComponent;
use crate::ui::components::working_animation_component::WorkingAnimationComponent;
use super_stt_shared::validation::get_http_socket_path;

/// Build the icon-alignment selector and activate the entry for the stored
/// [`IconAlignment`].
fn build_icon_alignment_model(
    active: IconAlignment,
) -> (SingleSelectModel, Entity, Entity, Entity) {
    let mut model = SingleSelectModel::default();
    let start = model.insert().text(IconAlignment::Start.pretty_name()).id();
    let center = model
        .insert()
        .text(IconAlignment::Center.pretty_name())
        .id();
    let end = model.insert().text(IconAlignment::End.pretty_name()).id();
    model.activate(match active {
        IconAlignment::Start => start,
        IconAlignment::Center => center,
        IconAlignment::End => end,
    });
    (model, start, center, end)
}

/// Build the color-config theme selector, activating the dark or light
/// entry to match the current system theme.
fn build_theme_selector_model(is_dark: bool) -> (SingleSelectModel, Entity, Entity) {
    let mut model = SingleSelectModel::default();
    let light = model.insert().text("Light Theme").id();
    let dark = model.insert().text("Dark Theme").id();
    if is_dark {
        model.activate(dark);
    } else {
        model.activate(light);
    }
    (model, light, dark)
}

impl SuperSttApplet {
    pub(super) fn new(
        core: cosmic::app::Core,
        visualization_side: VisualizationSide,
    ) -> (Self, cosmic_app::Task<Message>) {
        let variant_name = AppletConfig::get_variant_name(&visualization_side);
        let config = AppletConfig::load(variant_name, visualization_side.clone());

        let visualization = VisualizationComponent::new(
            0.0,
            false,
            config.visualization.theme.clone(),
            visualization_side.clone(),
            config.visualization.colors.clone(),
        );

        let working_animation = WorkingAnimationComponent::new(
            config.visualization.working_animation,
            visualization_side,
            config.visualization.colors.clone(),
        );

        let (icon_alignment_model, icon_alignment_start, icon_alignment_center, icon_alignment_end) =
            build_icon_alignment_model(config.ui.icon_alignment);

        let is_dark = cosmic::theme::active().cosmic().is_dark;
        let (theme_selector_model, theme_selector_light, theme_selector_dark) =
            build_theme_selector_model(is_dark);

        let applet = Self {
            core,
            recording_state: RecordingState::Idle,
            daemon_state: DaemonConnectionState::Connecting,
            popup: None,
            socket_path: get_http_socket_path(),
            audio_level: 0.0,
            is_speech_detected: false,
            is_open: IsOpen::None,
            udp_restart_counter: 0,
            visualization,
            working_animation,
            working_anim_start: None,
            config,
            icon_alignment_model,
            icon_alignment_start,
            icon_alignment_center,
            icon_alignment_end,
            theme_selector_model,
            theme_selector_light,
            theme_selector_dark,
            selected_theme_for_config: is_dark,
            retry_strategy: RetryStrategy::for_initial_connection(),
        };

        // Ping the daemon on startup. On failure, drop into the retry
        // loop rather than immediately surfacing an error.
        let initial_ping =
            cosmic_app::Task::perform(ping_daemon(applet.socket_path.clone()), |result| {
                cosmic::Action::App(match result {
                    Ok(_) => Message::DaemonConnected,
                    Err(e) => {
                        info!("Initial daemon connection failed: {e}");
                        Message::ScheduleRetry
                    }
                })
            });

        (applet, initial_ping)
    }
}
