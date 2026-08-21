// SPDX-License-Identifier: GPL-3.0-only
//! Updates page: current/latest version, automatic-check and beta-opt-in
//! settings, and the apply flow (download → spawn → JSON progress).

use cosmic::Element;
use cosmic::iced::widget::{column, row};
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, settings, text};

use super_stt_shared::models::self_update::SelfUpdateStatus;

use super::common::{error_banner, page_layout};
use super::models::{header_pill, pill_label, rounded_tooltip};
use crate::core::app::AppModel;
use crate::state::update::{RunPhase, UpdateState};
use crate::ui::messages::{Message, UpdateMessage};

const INSTALL_SH_URL: &str =
    "https://raw.githubusercontent.com/jorge-menjivar/super-stt/main/install.sh";

/// A tag is a prerelease iff it carries a semver `-<identifier>` suffix
/// (e.g. `v0.2.3-beta.1`) — the same rule `updater::run_update_stream` uses
/// to decide whether to pass `--beta` to the installer.
fn tag_is_prerelease(tag: &str) -> bool {
    tag.contains('-')
}

/// The curl-bootstrap fallback shown when the daemon reports an update but
/// published no installer asset for this host (unsupported arch, or the
/// release simply lacks one).
fn curl_fallback_caption(tag: &str) -> String {
    let beta_flag = if tag_is_prerelease(tag) {
        " -s -- --beta"
    } else {
        ""
    };
    format!(
        "Update available, but no installer asset was published for this system. Run: curl -sSL {INSTALL_SH_URL} | bash{beta_flag}"
    )
}

fn phase_label(phase: RunPhase) -> &'static str {
    match phase {
        RunPhase::FetchingInstaller => "Downloading installer…",
        RunPhase::Resolve => "Resolving…",
        RunPhase::Download => "Downloading update…",
        RunPhase::Verify => "Verifying…",
        RunPhase::Stage => "Staging…",
        RunPhase::WaitingAuth => {
            "Waiting for authorization — enter your password in the system dialog"
        }
        RunPhase::Install => "Installing…",
        RunPhase::PostInstall => "Finishing…",
        // Rendered through their own branches in `update_body`, never through
        // this label.
        RunPhase::Done | RunPhase::Failed => "",
    }
}

/// Version section: current/latest version, last-checked time (+ error
/// caption), and the "Check now" button.
fn version_section(state: &UpdateState) -> Element<'_, Message> {
    let status = state.status.as_ref();
    let latest = status
        .and_then(|s| s.latest_version.as_deref())
        .unwrap_or("—");
    let checked_at = status
        .and_then(|s| s.checked_at.as_deref())
        .unwrap_or("never");

    let mut checked_item = settings::item::builder("Last checked");
    if let Some(err) = status.and_then(|s| s.last_check_error.as_deref()) {
        checked_item = checked_item.description(err.to_string());
    }

    let busy = state.checking || state.run.is_some();
    let mut check_button = widget::button::standard("Check now");
    if !busy {
        check_button = check_button.on_press(Message::Update(UpdateMessage::CheckNow));
    }

    settings::section()
        .title("Version")
        .add(
            settings::item::builder("Current version")
                .control(text::body(env!("CARGO_PKG_VERSION"))),
        )
        .add(settings::item::builder("Latest version").control(text::body(latest.to_string())))
        .add(checked_item.control(text::body(checked_at.to_string())))
        .add(settings::item::builder("Check for updates").control(check_button))
        .into()
}

/// Settings section: the two togglers, both disabled while a run is active.
fn settings_section(state: &UpdateState) -> Element<'_, Message> {
    let run_active = state.run.is_some();
    let beta_effective = state
        .status
        .as_ref()
        .is_some_and(|s| s.beta_optin_effective);

    settings::section()
        .title("Settings")
        .add(
            settings::item::builder("Automatic update checks")
                .description("Periodically check for new releases and notify when one is found")
                .control(
                    widget::toggler(state.auto_check_enabled.unwrap_or(true)).on_toggle_maybe(
                        (!run_active)
                            .then_some(|b| Message::Update(UpdateMessage::AutoCheckToggled(b))),
                    ),
                ),
        )
        .add(
            settings::item::builder("Receive beta updates")
                .description("Consider prerelease versions when checking for updates")
                .control(
                    widget::toggler(beta_effective).on_toggle_maybe(
                        (!run_active)
                            .then_some(|b| Message::Update(UpdateMessage::BetaOptinToggled(b))),
                    ),
                ),
        )
        .into()
}

/// The dynamic content of the Update section's single row: the idle CTA, the
/// in-progress phase + byte progress + Cancel, the Done banner, or the
/// Failed error — all keyed off `state.run`.
// reason: byte-count → progress-bar fraction is intentionally lossy/cosmetic.
#[allow(clippy::cast_precision_loss)]
fn update_body<'a>(state: &'a UpdateState, status: &'a SelfUpdateStatus) -> Element<'a, Message> {
    let tag = status
        .latest_version
        .as_deref()
        .unwrap_or("the latest version");
    let spacing = cosmic::theme::spacing().space_xs;

    let Some(run) = state.run.as_ref() else {
        // Idle: the CTA, or (no published asset for this host) the curl
        // fallback caption in place of a live button.
        return if status.installer_asset.is_some() {
            widget::button::suggested(format!("Update to {tag}"))
                .on_press(Message::Update(UpdateMessage::StartUpdate))
                .into()
        } else {
            column![
                widget::button::suggested(format!("Update to {tag}")),
                text::caption(curl_fallback_caption(tag)),
            ]
            .spacing(spacing)
            .into()
        };
    };

    match run.phase {
        RunPhase::Done => {
            let mut col = column![text::caption("Update installed.")].spacing(spacing);
            if run.completed_components.iter().any(|c| c == "app") {
                col = col.push(
                    row![
                        text::body("Restart Super STT to finish the update"),
                        widget::button::suggested("Restart")
                            .on_press(Message::Update(UpdateMessage::RestartApp)),
                    ]
                    .align_y(Alignment::Center)
                    .spacing(spacing),
                );
            }
            col.into()
        }
        RunPhase::Failed => column![
            error_banner(run.error.as_deref().unwrap_or("Update failed")),
            text::caption(curl_fallback_caption(tag)),
        ]
        .spacing(spacing)
        .into(),
        phase => {
            let mut col = column![text::body(phase_label(phase))].spacing(spacing);
            if matches!(phase, RunPhase::FetchingInstaller | RunPhase::Download) {
                let fraction = (run.bytes_done as f32 / run.bytes_total.max(1) as f32).max(0.05);
                col = col.push(widget::determinate_linear(fraction).width(Length::Fill));
            }
            if run.cancellable() {
                col = col.push(
                    widget::button::destructive("Cancel")
                        .on_press(Message::Update(UpdateMessage::CancelUpdate)),
                );
            }
            col.into()
        }
    }
}

/// Update section: only rendered while `status.update_available`.
fn update_section<'a>(
    state: &'a UpdateState,
    status: &'a SelfUpdateStatus,
) -> Option<Element<'a, Message>> {
    status.update_available.then(|| {
        settings::section()
            .title("Update")
            .add(update_body(state, status))
            .into()
    })
}

/// Updates page: version info, automatic-check/beta togglers, and (when the
/// daemon reports one available) the update CTA / apply-flow progress.
pub fn page(state: &UpdateState) -> Element<'_, Message> {
    let mut blocks: Vec<Element<'_, Message>> = Vec::new();

    if let Some(message) = state.action_error.as_deref() {
        blocks.push(error_banner(message));
    }

    blocks.push(version_section(state));

    if state.unsupported {
        blocks.push(text::caption("The connected daemon predates update support.").into());
    } else {
        blocks.push(settings_section(state));
        if let Some(status) = state.status.as_ref()
            && let Some(update) = update_section(state, status)
        {
            blocks.push(update);
        }
    }

    page_layout("Updates", settings::view_column(blocks))
}

/// Header-bar badge shown while an update is available and no apply run is
/// in flight (once a run starts, the phase readout on the Updates page is
/// the source of truth — the badge would otherwise duplicate/contradict it).
/// Mirrors the GPU/status pills' construction (`ui/views/models/mod.rs`).
pub(crate) fn header_badge(app: &AppModel) -> Option<Element<'_, Message>> {
    if app.update.run.is_some() {
        return None;
    }
    if !app
        .update
        .status
        .as_ref()
        .is_some_and(|s| s.update_available)
    {
        return None;
    }

    let inner = row![
        crate::ui::icons::phosphor_tinted(
            crate::ui::icons::ARROWS_CLOCKWISE,
            14.0,
            cosmic::theme::active().cosmic().accent.base.into(),
        ),
        pill_label("Update available"),
    ]
    .spacing(6.0)
    .align_y(Alignment::Center);

    Some(rounded_tooltip(
        header_pill(inner),
        text::body("A new Super STT version is ready to install — see the Updates page"),
        widget::tooltip::Position::Bottom,
    ))
}
