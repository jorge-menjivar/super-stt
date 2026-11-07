// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::iced::widget::row;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::core::app::AppModel;
use crate::daemon::backends::BackendInfo;
use crate::state::ContextPage;
use crate::state::Stage;
use crate::ui::icons;
use crate::ui::messages::{Message, ShellMessage, StageMessage};

use super::active::backend_glyph_tile;
use super::surface::muted_text_color;

/// The Transcription section's empty state, shown when no backend is selected:
/// the same [`empty_state`](super::surface::empty_state) every stage has, with
/// the button that opens the [`select_backend_sheet`].
pub(super) fn no_backend_empty_state<'a>(narrow: bool) -> Element<'a, Message> {
    super::surface::empty_state(
        narrow,
        "No backend selected",
        "Select a backend to start transcribing.",
        Some(
            button::suggested("Select transcription backend")
                .leading_icon(icons::phosphor_handle(icons::PLAY))
                .on_press(Message::Shell(ShellMessage::ToggleContextPage(
                    ContextPage::SelectBackend,
                )))
                .into(),
        ),
    )
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
    // an empty model picker — and `POST /stt/pipeline/1` refuses it anyway.
    //
    // `None` is the answer still in flight; an empty answer is the daemon
    // saying nothing installed transcribes. The two read differently below.
    let answered = app.stage_catalog.backends(Stage::Transcription);
    let eligible: &[BackendInfo] = answered.unwrap_or_default();
    let mut col = widget::column::with_capacity(eligible.len() + 1)
        .spacing(spacing.space_xs)
        .width(Length::Fill)
        .push(
            text::caption(
                "Pick which backend powers transcription. Add and manage backends in your Library.",
            )
            .class(cosmic::theme::Text::Color(muted)),
        );

    if eligible.is_empty() {
        return col
            .push(text::body(if answered.is_none() {
                "Loading…"
            } else {
                "No installed backend provides a transcription model. Open the Library to \
                 install one."
            }))
            .into();
    }

    for backend in eligible {
        col = col.push(picker_row(
            app,
            Stage::Transcription,
            backend,
            active == Some(backend.source.as_str()),
        ));
    }
    col.into()
}

/// One backend's row in a stage's picker sheet, the same for every stage: the
/// glyph, the name, and the facts row its card will show (what it runs on,
/// how many models it offers this stage), then a "Selected" flag or the
/// Select button.
///
/// `backend` is the stage's narrowed entry (`/{feature}/pipeline/{n}/backend/list`),
/// so its models are the ones this stage can use.
pub(super) fn picker_row(
    app: &AppModel,
    stage: Stage,
    backend: &BackendInfo,
    is_selected: bool,
) -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();
    let mut meta = widget::column::with_capacity(2)
        .spacing(spacing.space_xxxs)
        .width(Length::Fill)
        .push(text::body(backend.name.clone()));
    if let Some(chips) = super::chips::stage_chips(app, stage, backend) {
        meta = meta.push(chips);
    }

    let trailing: Element<'static, Message> = if is_selected {
        text::caption("Selected")
            .class(cosmic::theme::Text::Accent)
            .into()
    } else {
        button::suggested("Select")
            .leading_icon(icons::phosphor_handle(icons::PLAY))
            .on_press(Message::Stage(StageMessage::SelectBackend {
                stage,
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
                    color: super::surface::accent_border_color(theme, is_selected),
                },
                ..Default::default()
            }
        }))
        .into()
}
