// SPDX-License-Identifier: GPL-3.0-only
mod active;
mod add_sheet;
mod chips;
mod configure;
mod download;
mod fmt;
mod installed;
mod language;
mod pipeline;
mod select_sheet;
mod stage_section;
mod status;
mod surface;
mod tabs;
mod voice;

use active::active_backend_card;
use download::download_split;
use installed::installed_tab;
use status::{ModelStatus, stage_status};
use surface::{bordered_scroll_view, muted_text_color, tab_bar_container, toolbar_container};
use tabs::models_tab_switcher;

use cosmic::Element;
use cosmic::iced::widget::row;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, text};

use super::common::page_container;
use crate::core::app::AppModel;
use crate::state::ModelsTab;
use crate::ui::icons;
use crate::ui::messages::Message;

pub use stage_section::{sheet as stage_sheet, sheet_title as stage_sheet_title};

pub use add_sheet::{add_backend_footer, add_backend_sheet};
pub use configure::configure_sheet;
pub use select_sheet::select_backend_sheet;

/// Re-exported so the Updates page's header badge (`ui/views/updates.rs`) can
/// give its tooltip the same small corner radius as the GPU/status pills'
/// instead of cosmic's default near-semicircular one.
pub(in crate::ui::views) use surface::rounded_tooltip;

/// Re-exported so the Updates page's header badge wears the same accent
/// styling as the Models/Library Update chip — one control, two places.
pub(in crate::ui::views) use surface::accent_button_class;

/// Wrap a header readout (GPU meter / status) in a neutral "pill": a soft
/// surface fill, hairline border, and fully-rounded corners, so the readouts
/// read as discrete indicators rather than text floating in the title bar.
///
/// These pills live in the window header bar (see `AppModel::header_end`),
/// whose content height is fixed and does NOT grow with the system spacing
/// setting. So the padding is fixed pixels, never `cosmic::theme::spacing()` —
/// theme-spaced padding would overflow the bar at generous spacing, compress
/// the pill, and squish round dots into ovals.
pub(crate) fn header_pill<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    widget::container(content.into())
        .padding([8, 12])
        .class(cosmic::theme::Container::custom(surface::pill_surface))
        .into()
}

/// A header-pill label: body-sized text with a relative line height of 1.0 so
/// its line box hugs the glyph. `text::body`'s taller fixed box would push the
/// text off-center from inline dots/meters and inflate the pill past the fixed
/// window-header height; this keeps every pill's contents the same height.
pub(crate) fn pill_label<'a>(
    content: impl Into<std::borrow::Cow<'a, str>> + 'a,
) -> Element<'a, Message> {
    widget::text(content).size(14.0).line_height(1.0).into()
}

/// [`pill_label`] in an explicit color, for a pill that is a control rather
/// than a readout: the accent-filled "Update available" badge tints its label
/// to match its icon and border. Shares the sizing so a tinted pill and a
/// neutral one still line up at the same height in the header bar.
pub(crate) fn pill_label_tinted<'a>(
    content: impl Into<std::borrow::Cow<'a, str>> + 'a,
    color: cosmic::iced::Color,
) -> Element<'a, Message> {
    widget::text(content)
        .size(14.0)
        .line_height(1.0)
        .class(cosmic::theme::Text::Color(color))
        .into()
}

/// The two pills in the window header, one per feature: speech to text's
/// microphone and text to speech's speaker, each with a dot in its state's
/// color. The hover tooltip names the feature and says what the color means,
/// so the indicator is not color-only.
///
/// A glyph and a dot, not words: two labelled pills would crowd the title bar.
/// The surface stays neutral, since a tinted pill reads as a button. Speech to
/// text's pill follows transcription, the stage that has to run for dictation
/// to type anything; text to speech's follows synthesis.
pub(crate) fn feature_status_pills(app: &AppModel) -> Element<'_, Message> {
    row![
        feature_pill(
            icons::MICROPHONE,
            "Speech to text",
            stage_status(app, crate::state::Stage::Transcription),
        ),
        feature_pill(
            icons::SPEAKER_HIGH,
            "Text to speech",
            stage_status(app, crate::state::Stage::Synthesis),
        ),
    ]
    .spacing(6.0)
    .align_y(Alignment::Center)
    .into()
}

fn feature_pill<'a>(
    glyph: &'static [u8],
    feature: &'static str,
    status: ModelStatus,
) -> Element<'a, Message> {
    // 14px like the GPU pill's glyph, so both pills keep the bar's height.
    let inner = row![icons::phosphor(glyph).size(14), status_dot(status, 10.0)]
        .spacing(6.0)
        .align_y(Alignment::Center);
    surface::rounded_tooltip(
        header_pill(inner),
        text::body(format!("{feature}: {}", status.detail())),
        widget::tooltip::Position::Bottom,
    )
}

/// A round dot in `status`'s color: gray for no backend, yellow for idle or
/// loading, green for ready, red for setup incomplete or not running.
fn status_dot<'a>(status: ModelStatus, size: f32) -> Element<'a, Message> {
    let color_fn: fn(&cosmic::Theme) -> cosmic::iced::Color = match status {
        ModelStatus::None => |t| t.cosmic().palette.neutral_5.into(),
        ModelStatus::Blocked | ModelStatus::Failed => |t| t.cosmic().destructive.base.into(),
        ModelStatus::Idle => |t| t.cosmic().warning.base.into(),
        ModelStatus::Ready => |t| t.cosmic().success.base.into(),
    };
    widget::container(widget::text(""))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .class(cosmic::theme::Container::custom(move |theme| {
            cosmic::iced::widget::container::Style {
                background: Some(cosmic::iced::Background::Color(color_fn(theme))),
                border: cosmic::iced::Border {
                    radius: (size / 2.0).into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }))
        .into()
}

/// A compact GPU readout for the header: a graphics-card glyph, the primary
/// GPU's (shortened) name + total memory, and a live usage meter — wrapped in a
/// [`header_pill`]. The hover tooltip lists every detected GPU with its
/// used/free memory and full name. `None` when the daemon reported no GPUs.
///
/// `compact` drops the name and figures, leaving the glyph and the meter, for a
/// window too narrow for both; the tooltip still has them.
pub(crate) fn gpu_summary(app: &AppModel, compact: bool) -> Option<Element<'_, Message>> {
    use fmt::{fmt_gib, fmt_gib_pair, short_gpu_name, vram_meter};
    use surface::rounded_tooltip;
    let primary = app.gpu_info.first()?;
    let name = short_gpu_name(&primary.name);
    // Live "used / total" when the daemon reports usage; total-only otherwise.
    let label = match primary.used_bytes {
        Some(used) => format!("{name} · {}", fmt_gib_pair(used, primary.total_bytes)),
        None => format!("{name} · {}", fmt_gib(primary.total_bytes)),
    };
    let detail = app
        .gpu_info
        .iter()
        .map(|g| match (g.used_bytes, g.free_bytes) {
            (Some(used), Some(free)) => format!(
                "{}\n{} used · {} free · {} total",
                g.name,
                fmt_gib(used),
                fmt_gib(free),
                fmt_gib(g.total_bytes),
            ),
            _ => format!("{}\n{} total", g.name, fmt_gib(g.total_bytes)),
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    // Inner row: a GPU glyph, the name + memory text, and a live used/total
    // meter bar when the daemon reports usage — all inside a header pill. The
    // tooltip carries the full per-GPU breakdown.
    let muted = muted_text_color();
    let mut inner = widget::row::with_capacity(3)
        .spacing(6.0)
        .align_y(Alignment::Center)
        .push(icons::phosphor_tinted(icons::GRAPHICS_CARD, 14.0, muted));
    if !compact {
        inner = inner.push(pill_label(label));
    }
    if let Some(used) = primary.used_bytes {
        inner = inner.push(vram_meter(used, primary.total_bytes));
    }
    Some(rounded_tooltip(
        header_pill(inner),
        text::body(detail),
        widget::tooltip::Position::Bottom,
    ))
}

// ── Models page ─────────────────────────────────────────────────────────────

/// Models page view: each feature's pipeline as a row of steps, and under the
/// rows the open step's card. Transcription's card is the active backend's, or
/// the empty state that opens the "Select a backend" sheet; post-processing's
/// and synthesis's come from [`stage_section`]. Installing and managing
/// backends lives on the Library page; selecting them happens here.
pub fn page(app: &AppModel) -> Element<'_, Message> {
    let title_row = widget::row::with_capacity(1)
        .align_y(Alignment::Center)
        .push(text::title3("Models").width(Length::Fill));

    let stage = app.models_page.pipeline_step;
    // Measured where the card is drawn, so a wide window with the side panel
    // open lays it out narrow too.
    let card = widget::responsive(move |size| {
        let width = size.width;
        match stage.block() {
            // The active backend's card when one is selected (and still
            // installed); otherwise the empty state that opens the "Select a
            // backend" sheet.
            None => match app
                .models_page
                .active_backend
                .as_deref()
                .and_then(|source| app.backends.iter().find(|b| b.source == source))
            {
                Some(backend) => active_backend_card(backend, app, width),
                None => select_sheet::no_backend_empty_state(),
            },
            Some(block) => stage_section::section(app, block, width),
        }
    })
    .height(Length::Shrink);

    // The rows and a card in a fixed-height column compete for it: the column
    // hands each child its share, and the loser is drawn clipped. Scrolling
    // gives each its natural height instead, so a short window scrolls rather
    // than truncates. Only the body scrolls; the page title stays put.
    let body = widget::column::with_capacity(4)
        .push(page_container(pipeline::pipelines(app)))
        .push(page_container(widget::divider::horizontal::default()))
        .push(page_container(section_heading(pipeline::stage_name(stage))))
        .push(page_container(card));

    widget::column::with_capacity(2)
        .push(page_container(title_row))
        .push(
            widget::scrollable(body)
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .height(Length::Fill)
        .spacing(0)
        .into()
}

/// The heading over the open step's card.
fn section_heading(label: &str) -> Element<'_, Message> {
    widget::row::with_capacity(1)
        .align_y(Alignment::Center)
        .push(text::title4(label).width(Length::Fill))
        .into()
}

/// Library page view: a fixed header (title + Installed/Browse tab bar) over a
/// scrollable list of backend cards. Installed cards manage and configure
/// backends (no activation); Browse installs new ones.
pub fn library_page(app: &AppModel) -> Element<'_, Message> {
    let active_tab = app
        .models_page
        .models_tabs
        .active_data::<ModelsTab>()
        .copied()
        .unwrap_or_default();

    let mut header = widget::column::with_capacity(3);
    let title_row = widget::row::with_capacity(1)
        .align_y(Alignment::Center)
        .push(text::title3("Library").width(Length::Fill));
    header = header.push(page_container(title_row));
    header = header.push(tab_bar_container(models_tab_switcher(app)));

    // Browse pins its search + filter toolbar above the scroll area; Installed
    // has no toolbar. Either way, only the card list scrolls.
    let tab_body = match active_tab {
        ModelsTab::Installed => installed_tab(app),
        ModelsTab::Download => {
            let (toolbar, cards) = download_split(app);
            header = header.push(toolbar_container(toolbar));
            cards
        }
    };

    widget::column::with_capacity(2)
        .push(header)
        .push(bordered_scroll_view(tab_body))
        .height(Length::Fill)
        .spacing(0)
        .into()
}
