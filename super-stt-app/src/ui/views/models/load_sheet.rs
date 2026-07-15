// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::iced::{Alignment, Length};
use cosmic::iced_widget::row;
use cosmic::widget::{self, button, text};

use crate::core::app::AppModel;
use crate::daemon::backends::BackendInfo;
use crate::state::ContextPage;
use crate::ui::icons;
use crate::ui::messages::{Message, ModelsPageMessage, ShellMessage};

use super::active::backend_glyph_tile;
use super::chips::{
    backend_is_online, backend_supports_cpu, backend_supports_gpu, capability_chips,
};
use super::surface::muted_text_color;

/// The Models-page empty state, shown when no backend is active: a soft glyph
/// tile, a short prompt, and a primary "Load a backend" button that opens the
/// [`load_backend_sheet`]. Fills the page so it sits centered.
pub(super) fn no_backend_empty_state<'a>() -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let ring = super::active::glyph_tile(72.0, 34.0, true);

    let col = widget::column::with_capacity(4)
        .align_x(Alignment::Center)
        .spacing(spacing.space_xs)
        .push(ring)
        .push(text::title4("No backend loaded"))
        .push(
            text::body("Load a backend to start transcribing.")
                .class(cosmic::theme::Text::Color(muted_text_color())),
        )
        .push(
            button::suggested("Load a backend")
                .leading_icon(icons::phosphor_handle(icons::PLAY))
                .on_press(Message::Shell(ShellMessage::ToggleContextPage(
                    ContextPage::LoadBackend,
                ))),
        );

    widget::container(col)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// The "Load a backend" side sheet: a hint line plus one row per installed
/// backend. The active backend is flagged; every other row carries a Load
/// button that activates it (and dismisses the sheet).
pub fn load_backend_sheet(app: &AppModel) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let muted = muted_text_color();
    let active = app.models_page.active_backend.as_deref();

    let mut col = widget::column::with_capacity(app.backends.len() + 1)
        .spacing(spacing.space_xs)
        .width(Length::Fill)
        .push(
            text::caption(
                "Pick which backend powers transcription. Add and manage backends in your Library.",
            )
            .class(cosmic::theme::Text::Color(muted)),
        );

    if app.backends.is_empty() {
        return col
            .push(text::body(
                "No backends installed yet. Open the Library to install one.",
            ))
            .into();
    }

    for backend in &app.backends {
        col = col.push(load_backend_row(
            backend,
            active == Some(backend.source.as_str()),
        ));
    }
    col.into()
}

/// One backend row inside the load sheet: glyph + name + capability chips, then
/// either an "Active" flag or a Load button.
fn load_backend_row(backend: &BackendInfo, is_active: bool) -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();
    let online = backend_is_online(backend);
    let hosts = online.then_some(backend.allowed_hosts.as_slice());

    let mut meta = widget::column::with_capacity(2)
        .spacing(spacing.space_xxxs)
        .width(Length::Fill)
        .push(text::body(backend.name.clone()));
    if let Some(chips) = capability_chips(
        backend_supports_gpu(backend),
        backend_supports_cpu(backend),
        hosts,
        true,
    ) {
        meta = meta.push(chips);
    }

    let trailing: Element<'static, Message> = if is_active {
        text::caption("Active")
            .class(cosmic::theme::Text::Accent)
            .into()
    } else {
        button::suggested("Load")
            .leading_icon(icons::phosphor_handle(icons::PLAY))
            .on_press(Message::ModelsPage(ModelsPageMessage::SelectBackend(
                backend.source.clone(),
            )))
            .into()
    };

    let inner = row![backend_glyph_tile(), meta, trailing]
        .spacing(spacing.space_s)
        .align_y(Alignment::Center);

    widget::container(inner)
        .padding(spacing.space_xs)
        .width(Length::Fill)
        .class(cosmic::theme::Container::custom(move |theme| {
            let cosmic = theme.cosmic();
            let component = &theme.current_container().component;
            cosmic::iced_widget::container::Style {
                background: Some(cosmic::iced::Background::Color(component.base.into())),
                border: cosmic::iced::Border {
                    radius: cosmic.corner_radii.radius_s.into(),
                    width: 1.0,
                    color: super::surface::accent_border_color(theme, is_active),
                },
                ..Default::default()
            }
        }))
        .into()
}
