// SPDX-License-Identifier: GPL-3.0-only
//! Updates page: current/latest version, automatic-check and beta-opt-in
//! settings, and the apply flow (download → spawn → JSON progress).

use cosmic::Element;
use cosmic::iced::widget::{column, row};
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, settings, text};

use super_stt_shared::models::self_update::SelfUpdateStatus;

use super::common::{error_banner, page_layout};
use super::models::{accent_button_class, pill_label_tinted, rounded_tooltip};
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

/// The candidate tag to show/build messages around, falling back to a
/// generic label when `status` is absent (a `Failed`/idle run can render
/// while a fresh status hasn't loaded, e.g. right after `DismissRun`).
fn tag_of(status: Option<&SelfUpdateStatus>) -> &str {
    status
        .and_then(|s| s.latest_version.as_deref())
        .unwrap_or("the latest version")
}

/// The curl-bootstrap command, with the `--beta` flag a prerelease tag needs.
/// Shared by the two captions below so the command itself is written once.
fn curl_bootstrap(tag: &str) -> String {
    let beta_flag = if tag_is_prerelease(tag) {
        " -s -- --beta"
    } else {
        ""
    };
    format!("curl -sSL {INSTALL_SH_URL} | bash{beta_flag}")
}

/// The curl-bootstrap fallback shown when the daemon reports an update but
/// published no installer asset for this host (unsupported arch, or the
/// release simply lacks one). There is no button to press in that case, and
/// this stands in for it.
fn curl_fallback_caption(tag: &str) -> String {
    format!(
        "Update available, but no installer asset was published for this system. Run: {}",
        curl_bootstrap(tag)
    )
}

/// The way forward offered after a run that *failed*.
///
/// Distinct from [`curl_fallback_caption`], which explains an update that
/// never had a button: reaching a failure means an asset was published,
/// downloaded and verified, so saying none was published is simply false —
/// and it was, on the panel a dismissed authorization prompt produced.
fn manual_install_caption(tag: &str) -> String {
    format!(
        "To install it outside the app, run: {}",
        curl_bootstrap(tag)
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

    let mut check_button = widget::button::standard("Check now");
    if state.can_check_now() {
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
                    // Inert while the write and its re-check are in flight:
                    // the switch has already moved, and a second press would
                    // race the first.
                    widget::toggler(state.beta_optin_shown()).on_toggle_maybe(
                        (!run_active && !state.beta_pending)
                            .then_some(|b| Message::Update(UpdateMessage::BetaOptinToggled(b))),
                    ),
                ),
        )
        .into()
}

/// The dynamic content of the Update section's single row: the idle CTA
/// (gated on `update_available` and an installer asset), the in-progress
/// phase and byte progress with Cancel, the Done banner, or the Failed
/// error — keyed off `state.run` first, `status` only for the idle
/// CTA/fallback text.
///
/// `run` always wins over the idle CTA, regardless of what `status` says
/// right now: a `Done`/`Failed` run's own panel (Restart/Dismiss, or the
/// error + Dismiss) must stay visible even after a post-update refetch
/// reports `update_available: false` — see `update_section`'s gate and
/// `UpdateState::clear_run_on_status_refresh`.
// reason: byte-count → progress-bar fraction is intentionally lossy/cosmetic.
#[allow(clippy::cast_precision_loss)]
fn update_body<'a>(
    state: &'a UpdateState,
    status: Option<&'a SelfUpdateStatus>,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing().space_xs;
    let tag = tag_of(status);

    let Some(run) = state.run.as_ref() else {
        // Idle: the CTA, or (no published asset for this host) the curl
        // fallback caption in place of a live button. `update_section` only
        // reaches this branch (no run) when `update_available` was true, so
        // `status` is always populated here.
        if status.is_none() {
            return text::body("").into();
        }
        return if state.can_start_update() {
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
                // A failed relaunch attempt (RestartApp's spawn erred) is
                // surfaced here rather than silently exiting the app with
                // nothing left running — see handlers/update.rs::RestartApp.
                if let Some(err) = run.error.as_deref() {
                    col = col.push(error_banner(err));
                }
            }
            col.push(dismiss_button()).into()
        }
        RunPhase::Failed => {
            let mut col = column![error_banner(
                run.error.as_deref().unwrap_or("Update failed")
            )]
            .spacing(spacing);
            // Declining the prompt is not a dead end — pressing Update again
            // and authorizing is the answer, so a shell command here would
            // read as a non-sequitur to someone who just pressed Cancel.
            if !run.was_authorization_declined() {
                col = col.push(text::caption(manual_install_caption(tag)));
            }
            col.push(dismiss_button()).into()
        }
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

/// The "Dismiss" button shown on a terminal (`Done`/`Failed`) run's panel —
/// clears it without restarting, so the page returns to the idle CTA and a
/// future `StartUpdate` isn't permanently blocked.
fn dismiss_button<'a>() -> Element<'a, Message> {
    widget::button::standard("Dismiss")
        .on_press(Message::Update(UpdateMessage::DismissRun))
        .into()
}

/// Update section: rendered whenever there's an in-flight/terminal run to
/// show (independent of the current `update_available` flag — a `Done` run's
/// Restart CTA must survive the post-update refetch that flips it to
/// `false`), or the daemon currently reports an update available.
fn update_section<'a>(
    state: &'a UpdateState,
    status: Option<&'a SelfUpdateStatus>,
) -> Option<Element<'a, Message>> {
    if !state.update_section_visible() {
        return None;
    }
    Some(
        settings::section()
            .title("Update")
            .add(update_body(state, status))
            .into(),
    )
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
        if let Some(update) = update_section(state, state.status.as_ref()) {
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
    if !app.update.header_badge_visible() {
        return None;
    }

    let fg: cosmic::iced::Color = cosmic::theme::active().cosmic().accent.base.into();
    let inner = row![
        crate::ui::icons::phosphor_tinted(crate::ui::icons::ARROWS_CLOCKWISE, 14.0, fg),
        pill_label_tinted("Update available", fg),
    ]
    .spacing(6.0)
    .align_y(Alignment::Center);

    // A button, wearing the same accent styling as the backend Update chip:
    // it is the same kind of control — an available version, and the way to
    // get to it. As a bare pill it looked pressable and wasn't.
    //
    // The padding is [`header_pill`]'s fixed pixels, not theme spacing, for
    // the reason given there: the header bar's height does not grow with the
    // system spacing setting.
    let badge = widget::button::custom(inner)
        .padding([8, 12])
        .class(accent_button_class(
            cosmic::theme::active().cosmic().corner_radii.radius_xl,
        ))
        .on_press(Message::Update(UpdateMessage::OpenUpdatesPage));

    Some(rounded_tooltip(
        badge,
        text::body("A new Super STT version is ready to install — open the Updates page"),
        widget::tooltip::Position::Bottom,
    ))
}

#[cfg(test)]
mod tests {
    use super::{curl_fallback_caption, manual_install_caption, tag_is_prerelease};

    #[test]
    fn a_prerelease_tag_carries_the_beta_flag_into_both_captions() {
        assert!(tag_is_prerelease("v0.2.3-beta.1"));
        assert!(!tag_is_prerelease("v0.2.3"));

        assert!(manual_install_caption("v0.2.3-beta.1").ends_with("| bash -s -- --beta"));
        assert!(manual_install_caption("v0.2.3").ends_with("| bash"));
        assert!(curl_fallback_caption("v0.2.3-beta.1").ends_with("| bash -s -- --beta"));
        assert!(curl_fallback_caption("v0.2.3").ends_with("| bash"));
    }

    /// The regression: a run that FAILED had reported "no installer asset was
    /// published for this system", on a panel produced by dismissing the
    /// authorization prompt. Reaching a failure means an asset was published,
    /// downloaded and verified — the claim is false wherever a run got far
    /// enough to fail, so the two captions must not be interchangeable.
    #[test]
    fn only_the_idle_caption_claims_no_asset_was_published() {
        let idle = curl_fallback_caption("v0.2.3");
        let failed = manual_install_caption("v0.2.3");

        assert!(idle.contains("no installer asset was published"));
        assert!(
            !failed.contains("no installer asset was published"),
            "a failed run proves an asset existed: {failed}"
        );
        assert!(failed.contains("To install it outside the app"));
    }
}
