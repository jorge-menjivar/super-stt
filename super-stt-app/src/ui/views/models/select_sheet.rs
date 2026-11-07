// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::iced::widget::row;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::core::app::AppModel;
use crate::daemon::backends::BackendInfo;
use crate::state::ContextPage;
use crate::state::device_offers::STT_STAGE;
use crate::ui::icons;
use crate::ui::messages::{Message, ShellMessage, StageMessage};

use super::active::backend_glyph_tile;
use super::chips::{
    CloudEgress, backend_has_user_url, backend_is_online, backend_supports_cpu,
    backend_supports_gpu, capability_chips,
};
use super::surface::muted_text_color;

/// The Transcription section's empty state, shown when no backend is selected:
/// a soft glyph tile, a short prompt, and a primary "Select a backend" button
/// that opens the [`select_backend_sheet`].
///
/// Centered horizontally but sized to its content — it is one section of two,
/// and filling the page height would push the Post-processing heading to the
/// bottom of the window.
pub(super) fn no_backend_empty_state<'a>() -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let ring = super::active::glyph_tile(72.0, 34.0, true);

    let col = widget::column::with_capacity(4)
        .align_x(Alignment::Center)
        .spacing(spacing.space_xs)
        .push(ring)
        .push(text::title4(crate::fl!("no-backend-selected")))
        .push(
            text::body(crate::fl!("select-a-backend-to-start-transcribing"))
                .class(cosmic::theme::Text::Color(muted_text_color())),
        )
        .push(
            button::suggested(crate::fl!("select-transcription-backend"))
                .leading_icon(icons::phosphor_handle(icons::PLAY))
                .on_press(Message::Shell(ShellMessage::ToggleContextPage(
                    ContextPage::SelectBackend,
                ))),
        );

    widget::container(col)
        .center_x(Length::Fill)
        .width(Length::Fill)
        .padding([spacing.space_m, 0, spacing.space_m, 0])
        .into()
}

/// The "Select transcription backend" side sheet: a hint line plus one row per installed
/// backend. The active backend is flagged; every other row carries a Select
/// button that activates it (and dismisses the sheet).
pub fn select_backend_sheet(app: &AppModel) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let muted = muted_text_color();
    let active = app.models_page.active_backend.as_deref();

    // What the daemon says can fill stage 1, not a filter of the catalog: a
    // backend serving nothing but post-processors would be selected, then show
    // an empty model picker — and `POST /pipeline/1` refuses it anyway.
    //
    // `None` is the answer still in flight; an empty answer is the daemon
    // saying nothing installed transcribes. The two read differently below.
    let answered = app.stage_catalog.backends(STT_STAGE);
    let eligible: &[BackendInfo] = answered.unwrap_or_default();
    let mut col = widget::column::with_capacity(eligible.len() + 1)
        .spacing(spacing.space_xs)
        .width(Length::Fill)
        .push(
            text::caption(crate::fl!(
                "pick-which-backend-powers-transcription-add-and"
            ))
            .class(cosmic::theme::Text::Color(muted)),
        );

    if eligible.is_empty() {
        return col
            .push(text::body(if answered.is_none() {
                crate::fl!("loading-progress")
            } else {
                crate::fl!("no-installed-backend-provides-a-transcription-model")
            }))
            .into();
    }

    for backend in eligible {
        col = col.push(select_backend_row(
            backend,
            active == Some(backend.source.as_str()),
        ));
    }
    col.into()
}

/// One backend row inside the sheet: glyph + name + capability chips, then
/// either an "Active" flag or a Select button.
fn select_backend_row(backend: &BackendInfo, is_active: bool) -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();
    let online = backend_is_online(backend);
    let egress = online.then(|| CloudEgress {
        hosts: backend.allowed_hosts.as_slice(),
        user_url: backend_has_user_url(backend),
    });

    let mut meta = widget::column::with_capacity(2)
        .spacing(spacing.space_xxxs)
        .width(Length::Fill)
        .push(text::body(backend.name.clone()));
    if let Some(chips) = capability_chips(
        backend_supports_gpu(backend),
        backend_supports_cpu(backend),
        egress,
        true,
    ) {
        meta = meta.push(chips);
    }

    let trailing: Element<'static, Message> = if is_active {
        text::caption(crate::fl!("active"))
            .class(cosmic::theme::Text::Accent)
            .into()
    } else {
        button::suggested(crate::fl!("select"))
            .leading_icon(icons::phosphor_handle(icons::PLAY))
            .on_press(Message::Stage(StageMessage::SelectBackend {
                stage: STT_STAGE,
                source: backend.source.clone(),
            }))
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
            cosmic::iced::widget::container::Style {
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
