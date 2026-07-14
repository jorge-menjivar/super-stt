// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::iced::Length;
use cosmic::widget::{self, button, text};

use crate::core::app::AppModel;
use crate::ui::messages::{Message, ModelsPageMessage};

/// Tab switcher for the Models page (Installed / Browse): flat text tabs with an
/// accent border marking the active one, over a full-width hairline divider.
/// Replaces the native `tab_bar`'s raised-button look, which read as separate
/// buttons.
///
/// Still driven by `app.models_tabs`, so clicking a tab emits the same
/// [`ModelsPageMessage::ModelsTabActivated`] the segmented-button model expects.
pub(super) fn models_tab_switcher(app: &AppModel) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let active = app.models_tabs.active();

    let mut row = widget::row::with_capacity(2).spacing(spacing.space_xxs);
    for entity in app.models_tabs.iter().collect::<Vec<_>>() {
        let is_active = entity == active;
        let label = app
            .models_tabs
            .text(entity)
            .map(ToOwned::to_owned)
            .unwrap_or_default();

        let label_text = if is_active {
            text::body(label).class(cosmic::theme::Text::Accent)
        } else {
            text::body(label)
        };

        let inner = button::custom(label_text)
            .padding([spacing.space_xxs, spacing.space_s])
            .on_press(Message::ModelsPage(ModelsPageMessage::ModelsTabActivated(
                entity,
            )))
            .class(tab_inner_class(is_active));

        // Accent border on the top and sides only (open at the bottom): an
        // accent-filled layer revealed as a 1px rim by per-side padding (0 at the
        // bottom), with the inner button masking the centre. iced borders are
        // uniform, so this fill+mask is the only way to get a 3-sided, square-
        // bottomed, top-rounded tab outline.
        row = row.push(
            widget::container(inner)
                .padding(cosmic::iced::Padding {
                    top: 1.0,
                    right: 1.0,
                    bottom: 0.0,
                    left: 1.0,
                })
                .class(cosmic::theme::Container::custom(move |theme| {
                    let cosmic = theme.cosmic();
                    let r = cosmic.corner_radii.radius_s;
                    let bg = if is_active {
                        cosmic.accent.base.into()
                    } else {
                        cosmic::iced::Color::TRANSPARENT
                    };
                    cosmic::iced_widget::container::Style {
                        background: Some(cosmic::iced::Background::Color(bg)),
                        border: cosmic::iced::Border {
                            radius: [r[0], r[1], 0.0, 0.0].into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                })),
        );
    }

    widget::column::with_capacity(2)
        .push(row)
        .push(widget::divider::horizontal::default())
        .width(Length::Fill)
        .into()
}

/// Click + fill styling for the inner surface of a Models page tab. The accent
/// outline itself lives on the wrapping container (top + sides only); this layer
/// masks that outline down to its 1px rim — the selected tab fills with the page
/// background so only the rim shows, unselected tabs stay transparent. Hover and
/// press use the standard button hover/press fill so the tabs feel like the rest
/// of the app's buttons. Top corners rounded, bottom corners square, to match the
/// open-bottomed outline.
pub(super) fn tab_inner_class(active: bool) -> cosmic::theme::Button {
    fn shape(theme: &cosmic::Theme, fill: cosmic::iced::Color) -> cosmic::widget::button::Style {
        let r = theme.cosmic().corner_radii.radius_s;
        cosmic::widget::button::Style {
            background: Some(cosmic::iced::Background::Color(fill)),
            border_radius: [r[0], r[1], 0.0, 0.0].into(),
            text_color: Some(theme.current_container().component.on.into()),
            ..Default::default()
        }
    }
    // Idle masks the accent layer: the selected tab fills with the page
    // background (current container base) so only the 1px rim shows; others stay
    // transparent.
    let idle_fill = move |theme: &cosmic::Theme| {
        if active {
            theme.current_container().base.into()
        } else {
            cosmic::iced::Color::TRANSPARENT
        }
    };
    cosmic::theme::Button::Custom {
        active: Box::new(move |_focused: bool, theme| shape(theme, idle_fill(theme))),
        disabled: Box::new(move |theme| shape(theme, idle_fill(theme))),
        hovered: Box::new(|_focused: bool, theme| shape(theme, theme.cosmic().button.hover.into())),
        pressed: Box::new(|_focused: bool, theme| {
            shape(theme, theme.cosmic().button.pressed.into())
        }),
    }
}
