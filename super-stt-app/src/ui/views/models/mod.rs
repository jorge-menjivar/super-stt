// SPDX-License-Identifier: GPL-3.0-only
mod active;
mod add_sheet;
mod chips;
mod configure;
mod download;
mod fmt;
mod installed;
mod status;
mod surface;
mod tabs;

use active::active_backend_card;
use download::download_split;
use installed::installed_tab;
use status::{ModelStatus, model_status};
use surface::{bordered_scroll_view, muted_text_color, tab_bar_container, toolbar_container};
use tabs::models_tab_switcher;

use cosmic::Element;
use cosmic::iced::{Alignment, Length};
use cosmic::iced_widget::row;
use cosmic::widget::{self, text};

use super::common::page_container;
use crate::core::app::AppModel;
use crate::state::ModelsTab;
use crate::ui::icons;
use crate::ui::messages::Message;

pub use add_sheet::add_backend_sheet;
pub use configure::configure_sheet;

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
        .class(cosmic::theme::Container::custom(|theme| {
            let cosmic = theme.cosmic();
            let component = &theme.current_container().component;
            cosmic::iced_widget::container::Style {
                background: Some(cosmic::iced::Background::Color(component.base.into())),
                border: cosmic::iced::Border {
                    radius: cosmic.corner_radii.radius_xl.into(),
                    width: 1.0,
                    color: component.divider.into(),
                },
                ..Default::default()
            }
        }))
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

/// A pill in the window header showing model readiness: a small colored dot
/// (gray = none → red = blocked → yellow = idle → green = ready) and a short
/// label, over the neutral [`header_pill`] surface. The hover tooltip carries
/// the longer description so the indicator isn't color-only.
pub(crate) fn status_pill(app: &AppModel) -> Element<'_, Message> {
    use surface::rounded_tooltip;
    let (color_fn, short, detail): (fn(&cosmic::Theme) -> cosmic::iced::Color, &str, &str) =
        match model_status(app) {
            ModelStatus::None => (
                |t| t.cosmic().palette.neutral_5.into(),
                "No backend",
                "No backend selected",
            ),
            ModelStatus::Blocked => (
                |t| t.cosmic().destructive.base.into(),
                "Incomplete",
                "Backend configuration incomplete",
            ),
            ModelStatus::Idle => (
                |t| t.cosmic().warning.base.into(),
                "Idle",
                "Backend ready — pick a model",
            ),
            ModelStatus::Ready => (
                |t| t.cosmic().success.base.into(),
                "Ready",
                "Model loaded and ready",
            ),
        };

    let size = 10.0_f32;
    let dot = widget::container(widget::text(""))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .class(cosmic::theme::Container::custom(move |theme| {
            cosmic::iced_widget::container::Style {
                background: Some(cosmic::iced::Background::Color(color_fn(theme))),
                border: cosmic::iced::Border {
                    radius: (size / 2.0).into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }));

    let inner = row![dot, pill_label(short)]
        .spacing(6.0)
        .align_y(Alignment::Center);

    rounded_tooltip(
        header_pill(inner),
        text::body(detail),
        widget::tooltip::Position::Bottom,
    )
}

/// A compact GPU readout for the header: a graphics-card glyph, the primary
/// GPU's (shortened) name + total memory, and a live usage meter — wrapped in a
/// [`header_pill`]. The hover tooltip lists every detected GPU with its
/// used/free memory and full name. `None` when the daemon reported no GPUs.
pub(crate) fn gpu_summary(app: &AppModel) -> Option<Element<'_, Message>> {
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
        .push(icons::phosphor_tinted(icons::GRAPHICS_CARD, 14.0, muted))
        .push(pill_label(label));
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

/// Models page view: a fixed header (the active-backend card, when one is
/// selected, plus the Installed/Download tab bar) over a scrollable list — or
/// the per-backend configuration sub-view when one is open.
pub fn page(app: &AppModel) -> Element<'_, Message> {
    // Per-backend configuration now opens as a right-side sheet (see
    // `configure_sheet` + `AppModel::context_drawer`) rather than taking over
    // the page, so this view always renders the backend list.
    let active_tab = app
        .models_tabs
        .active_data::<ModelsTab>()
        .copied()
        .unwrap_or_default();

    // Fixed header: the title, the active-backend card (when a backend is
    // selected), then the tab bar. Only the list below scrolls. The GPU summary
    // and model-readiness pills now live in the window header bar (see
    // `AppModel::header_end`) rather than this title row.
    let mut header = widget::column::with_capacity(3);
    let title_row = widget::row::with_capacity(1)
        .align_y(Alignment::Center)
        .push(text::title3("Models").width(Length::Fill));
    header = header.push(page_container(title_row));
    if let Some(source) = &app.active_backend
        && let Some(backend) = app.backends.iter().find(|b| &b.source == source)
    {
        header = header.push(page_container(active_backend_card(backend, app)));
    }
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
