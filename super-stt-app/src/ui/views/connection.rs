// SPDX-License-Identifier: GPL-3.0-only
use super::common::page_layout;
use crate::core::app::AppModel;
use crate::state::DaemonStatus;
use crate::ui::messages::{DaemonMessage, Message, ShellMessage};
use cosmic::iced::Alignment;
use cosmic::iced::widget::row;
use cosmic::{
    Element,
    widget::{button, settings, text},
};

/// What fixes a daemon that is installed but not running — which is the only
/// way this page has anything to say, since the app and the daemon install
/// together.
///
/// Built at call time rather than held in a `const`, because the launchd form
/// needs the caller's uid: `launchctl` addresses a user agent by domain
/// target (`gui/<uid>/<label>`), never by label alone.
fn start_daemon_command() -> String {
    #[cfg(target_os = "linux")]
    {
        "systemctl --user start super-stt".to_string()
    }
    #[cfg(not(target_os = "linux"))]
    {
        format!("launchctl kickstart {}", launchd_target())
    }
}

/// The same, for a daemon that is running and needs replacing. `-k` kills the
/// current process first; plain `kickstart` on a running agent is a no-op,
/// which would leave the deny cache the message is telling them to clear.
fn restart_daemon_command() -> String {
    #[cfg(target_os = "linux")]
    {
        "systemctl --user restart super-stt".to_string()
    }
    #[cfg(not(target_os = "linux"))]
    {
        format!("launchctl kickstart -k {}", launchd_target())
    }
}

/// The `LaunchAgent`'s domain target. Matches the label in
/// `super-stt-daemon/launchd/` and the `launchd_target` the justfile builds.
#[cfg(not(target_os = "linux"))]
fn launchd_target() -> String {
    format!(
        "gui/{}/{}",
        unsafe { libc::getuid() },
        super_stt_shared::launch_agents::Agent::Daemon.label()
    )
}

/// Settings page view using cosmic-settings style
pub fn page(app: &AppModel) -> Element<'_, Message> {
    let daemon_status = &app.daemon_status;
    let status_text = match daemon_status {
        DaemonStatus::Connected => "✅ Connected".to_string(),
        DaemonStatus::Connecting => "⏳ Connecting...".to_string(),
        DaemonStatus::Disconnected => "❌ Disconnected".to_string(),
        DaemonStatus::Error(err) => format!("❌ Error: {err}"),
        DaemonStatus::Blocked(reason) => format!("⛔ Authorization denied ({reason})"),
    };

    let mut connection_section = settings::section()
        .title("Connection Information")
        .add(settings::item("Connection", text::body(status_text)))
        .add(settings::item(
            "Socket Path",
            text::body(app.socket_path.to_string_lossy().to_string()),
        ));

    // Not reaching the daemon is the one failure this page can actually help
    // with, and it never said how. A red status and nothing to act on is not
    // an answer, so offer what fixes it.
    if matches!(
        daemon_status,
        DaemonStatus::Disconnected | DaemonStatus::Error(_)
    ) {
        connection_section = not_running(connection_section, app);
    }

    if matches!(daemon_status, DaemonStatus::Blocked(_)) {
        connection_section = connection_section
            .add(settings::item(
                "Action required",
                text::body(format!(
                    "Authorization was denied. Restart the daemon to clear the deny \
                     cache ({}), then click Retry to request access again.",
                    restart_daemon_command()
                )),
            ))
            .add(settings::item(
                "",
                button::standard("Retry authorization")
                    .on_press(Message::Daemon(DaemonMessage::RetryAuthorization)),
            ));
    }

    let sections = vec![connection_section.into()];

    let sections_view = settings::view_column(sections);
    page_layout("Connection", sections_view)
}

/// What to do about a daemon that cannot be reached: usually the command
/// that starts it, ready to paste.
fn not_running<'a>(
    section: settings::Section<'a, Message>,
    #[cfg_attr(
        not(target_os = "macos"),
        expect(unused_variables, reason = "only macOS reads the agent's status")
    )]
    app: &AppModel,
) -> settings::Section<'a, Message> {
    // From the app bundle, launchd runs the daemon only while its agent is
    // registered and switched on, and a launchctl command fixes neither.
    #[cfg(target_os = "macos")]
    match app.daemon_agent {
        Some(super_stt_shared::launch_agents::Status::RequiresApproval) => {
            return section
                .add(settings::item(
                    "No daemon",
                    text::body(
                        "Super STT's background service is switched off in System \
                         Settings › General › Login Items.",
                    ),
                ))
                .add(settings::item(
                    "Turn it on",
                    button::standard("Open Login Items")
                        .on_press(Message::Shell(ShellMessage::OpenLoginItems)),
                ));
        }
        Some(super_stt_shared::launch_agents::Status::NotRegistered) => {
            return section
                .add(settings::item(
                    "No daemon",
                    text::body("Super STT's background service is not registered with macOS."),
                ))
                .add(settings::item(
                    "Register it",
                    button::standard("Register")
                        .on_press(Message::Shell(ShellMessage::RegisterAgents)),
                ));
        }
        _ => {}
    }

    let start_command = start_daemon_command();
    section
        .add(settings::item(
            "No daemon",
            text::body("The daemon is not running."),
        ))
        .add(settings::item(
            "Start it",
            row![
                text::body(start_command.clone()),
                button::standard("Copy")
                    .on_press(Message::Shell(ShellMessage::CopyText(start_command))),
            ]
            .spacing(cosmic::theme::spacing().space_xs)
            .align_y(Alignment::Center),
        ))
}
