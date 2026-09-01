// SPDX-License-Identifier: GPL-3.0-only
use cosmic::iced::Length;
use cosmic::iced::widget::row;
use cosmic::widget::{self, text};
use cosmic::{Apply, Element};

use crate::core::app::AppModel;
use crate::ui::icons;
use crate::ui::messages::{Message, ShellMessage};

/// Wrap the Models page's tab body in a bordered, page-width frame: the
/// scrollable list sits *inside* the border, so the outline stays fixed while
/// the cards scroll within it. Mirrors [`page_container`]'s centering and
/// width so the frame lines up with the header boxes above it.
pub(super) fn bordered_scroll_view<'a>(
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();

    // All the breathing room lives *inside* the scrollable (on the content),
    // not on the frame around it. That keeps the first/last cards off the
    // top/bottom edges and leaves a right-hand gutter for the scrollbar, so it
    // sits beside the cards instead of on top of them. (Padding the frame
    // instead insets the whole viewport — scrollbar included — leaving the bar
    // over the full-width cards.)
    let list = widget::container(content.into()).padding(spacing.space_s);

    let framed = widget::scrollable(list)
        .height(Length::Fill)
        .apply(widget::container)
        .max_width(800)
        .width(Length::Fill)
        .height(Length::Fill)
        .class(cosmic::theme::Container::custom(|theme| {
            let component = &theme.current_container().component;
            cosmic::iced::widget::container::Style {
                border: cosmic::iced::Border {
                    radius: theme.cosmic().corner_radii.radius_m.into(),
                    width: 1.0,
                    color: component.divider.into(),
                },
                ..Default::default()
            }
        }));

    widget::container(framed)
        .center_x(Length::Fill)
        .padding([0, spacing.space_l, spacing.space_m, spacing.space_l])
        .height(Length::Fill)
        .into()
}

/// The tab bar's container: mirrors [`page_container`]'s centering and side
/// padding but with a smaller bottom gap, so the Installed/Download tabs sit
/// close to the bordered list below them.
pub(super) fn tab_bar_container<'a>(
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    widget::container(content.into())
        .max_width(800)
        .width(Length::Fill)
        .apply(widget::container)
        .center_x(Length::Fill)
        .padding([0, spacing.space_l, spacing.space_xs, spacing.space_l])
        .into()
}

/// Centers the Browse toolbar (search + filters) to the same page width as the
/// scroll frame below it and adds a small gap, so it reads as a fixed header
/// above the scrolling card list rather than scrolling with the cards.
pub(super) fn toolbar_container<'a>(
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    widget::container(content.into())
        .max_width(800)
        .width(Length::Fill)
        .apply(widget::container)
        .center_x(Length::Fill)
        .padding([0, spacing.space_l, spacing.space_s, spacing.space_l])
        .into()
}

/// The border color a selectable panel takes: a translucent accent when
/// active, else the neutral divider. Shared by the active-backend card and the
/// load-sheet backend rows so "selected" reads the same everywhere.
pub(super) fn accent_border_color(theme: &cosmic::Theme, active: bool) -> cosmic::iced::Color {
    if active {
        let mut a: cosmic::iced::Color = theme.cosmic().accent.base.into();
        a.a = 0.55;
        a
    } else {
        theme.current_container().component.divider.into()
    }
}

/// The shared "pill" container style: list-container fill, a hairline divider
/// border, and an extra-large corner radius (no shadow). Used by the header
/// pills and the segmented-control track.
pub(super) fn pill_surface(theme: &cosmic::Theme) -> cosmic::iced::widget::container::Style {
    let cosmic = theme.cosmic();
    let component = &theme.current_container().component;
    cosmic::iced::widget::container::Style {
        background: Some(cosmic::iced::Background::Color(component.base.into())),
        border: cosmic::iced::Border {
            radius: cosmic.corner_radii.radius_xl.into(),
            width: 1.0,
            color: component.divider.into(),
        },
        ..Default::default()
    }
}

/// Wrap a card's content column in the shared card surface: a panel matching
/// the list-container fill, lifted with a soft border, rounded corners, and a
/// subtle shadow. The active backend's card takes an accent border to set it
/// apart from the installed list below it.
pub(super) fn card_surface<'a>(
    content: impl Into<Element<'a, Message>>,
    active: bool,
) -> Element<'a, Message> {
    widget::container(content.into())
        .padding(cosmic::theme::spacing().space_s)
        .width(Length::Fill)
        .class(cosmic::theme::Container::custom(move |theme| {
            let cosmic = theme.cosmic();
            let component = &theme.current_container().component;
            let border_color = accent_border_color(theme, active);
            cosmic::iced::widget::container::Style {
                icon_color: Some(component.on.into()),
                text_color: Some(component.on.into()),
                background: Some(cosmic::iced::Background::Color(component.base.into())),
                border: cosmic::iced::Border {
                    radius: cosmic.corner_radii.radius_m.into(),
                    width: 1.0,
                    color: border_color,
                },
                shadow: cosmic::iced::Shadow {
                    color: cosmic::iced::Color {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.12,
                    },
                    offset: cosmic::iced::Vector::new(0.0, 1.0),
                    blur_radius: 4.0,
                },
                snap: true,
            }
        }))
        .into()
}

/// The card header's "empty this stage" action: a ✕ on the destructive pill,
/// with a tooltip naming what it does.
///
/// An icon rather than the word "Deselect" because it sits third in a row of
/// actions and a ✕ reads as "clear this" at a glance. It keeps the destructive
/// background the labelled button had — that red is what marks this as the one
/// action in the row that throws something away, and a bare glyph would sit in
/// the row looking as harmless as Configure. The tooltip carries the label the
/// icon drops, so the action stays discoverable.
pub(super) fn deselect_button(tooltip_text: &str, message: Message) -> Element<'_, Message> {
    // The icon is left untinted so it takes the pill's foreground colour;
    // tinting it destructive would paint red on red.
    let button = widget::button::icon(icons::phosphor_handle(icons::X))
        .class(cosmic::theme::Button::Destructive)
        .on_press(message);
    rounded_tooltip(
        button,
        text::body(tooltip_text),
        cosmic::widget::tooltip::Position::Bottom,
    )
}

/// A faint full-width rule used inside cards to separate the header/body from a
/// footer action row.
pub(super) fn card_divider<'a>() -> Element<'a, Message> {
    widget::divider::horizontal::default().into()
}

/// The de-emphasized color for a card's secondary line (the repo id): the
/// container's text color dimmed so the technical `source` reads as a caption
/// rather than competing with the backend name.
pub(super) fn muted_text_color() -> cosmic::iced::Color {
    let mut c: cosmic::iced::Color = cosmic::theme::active()
        .current_container()
        .component
        .on
        .into();
    c.a = 0.65;
    c
}

/// One caption per kind of model a backend serves, each naming the kind and
/// then its models joined by " · " (e.g.
/// `"Speech to text: whisper-large-v3 · whisper-medium"`). De-emphasized so it
/// reads as a secondary detail under the card's description. `None` when the
/// backend serves no models.
///
/// Browse describes a backend the user has not installed, so which kinds it
/// ships is exactly what decides whether they want it.
pub(super) fn models_line<'a>(groups: &[super::chips::RoleGroup]) -> Option<Element<'a, Message>> {
    if groups.is_empty() {
        return None;
    }
    let muted = muted_text_color();
    let lines: Vec<Element<'a, Message>> = groups
        .iter()
        .map(|group| {
            text::caption(format!(
                "{}: {}",
                group.label,
                group.names.join(" \u{00b7} ")
            ))
            .class(cosmic::theme::Text::Color(muted))
            .into()
        })
        .collect();
    Some(
        cosmic::iced::widget::column(lines)
            .spacing(cosmic::theme::spacing().space_xxxs)
            .into(),
    )
}

/// A small icon button that opens a backend's source repository in the
/// browser, replacing the raw repo-id caption the cards used to show. `source`
/// is a bare repo id (e.g. `github.com/owner/name`), so the scheme is
/// prepended. Carries an "Open repository" tooltip.
pub(super) fn repo_button(source: &str) -> Element<'static, Message> {
    use crate::ui::icons;
    let url = format!("https://{source}");
    rounded_tooltip(
        widget::button::icon(icons::phosphor_handle(icons::GIT_BRANCH))
            .on_press(Message::Shell(ShellMessage::LaunchUrl(url))),
        text::body("Open repository"),
        widget::tooltip::Position::Top,
    )
}

/// A card's identity block: the backend name with an optional muted
/// description beneath it. Takes the remaining width so trailing actions sit
/// flush at the card's right edge.
pub(super) fn card_title_block<'a>(
    name: String,
    version: &str,
    description: Option<String>,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    // The version rides beside the name, muted: it answers "what am I running"
    // without competing with the name for the eye. Backends installed before
    // the daemon reported it have none, so the row simply omits it rather than
    // showing a placeholder.
    let title: Element<'a, Message> = if version.is_empty() {
        text::title4(name).line_height(1.0).into()
    } else {
        row![
            text::title4(name).line_height(1.0),
            text::body(format!("v{version}")).class(cosmic::theme::Text::Color(muted_text_color())),
        ]
        .spacing(spacing.space_xxs)
        .align_y(cosmic::iced::Alignment::End)
        .into()
    };
    let mut col = widget::column::with_capacity(2)
        .spacing(spacing.space_xxxs)
        .width(Length::Fill)
        .push(title);
    if let Some(desc) = description {
        col = col.push(text::body(desc).class(cosmic::theme::Text::Color(muted_text_color())));
    }
    col.into()
}

/// A backend's one-line description, looked up on its registry entry by
/// `source`. The installed `/backends` payload carries no description, so the
/// registry index is the source; `None` when it isn't loaded or has no
/// (non-empty) description for this backend.
pub(super) fn backend_description(app: &AppModel, source: &str) -> Option<String> {
    app.registry
        .by_source()
        .get(source)
        .and_then(|e| e.description.clone())
        .filter(|d| !d.is_empty())
}

/// A tooltip with a small (`radius_s` = 8 px) corner radius — cosmic's
/// default `Container::Tooltip` uses `radius_l` (32 px), which is almost
/// semicircular on a short row and reads as a pill. The padding/gap match
/// cosmic's default `tooltip()` helper.
pub(in crate::ui::views) fn rounded_tooltip<'a>(
    content: impl Into<Element<'a, Message>>,
    popup: impl Into<Element<'a, Message>>,
    position: cosmic::widget::tooltip::Position,
) -> Element<'a, Message> {
    let xxs = cosmic::theme::spacing().space_xxs;
    cosmic::widget::tooltip::Tooltip::new(content, popup, position)
        .class(cosmic::theme::Container::custom(|theme| {
            let cosmic = theme.cosmic();
            cosmic::iced::widget::container::Style {
                icon_color: None,
                text_color: None,
                background: Some(cosmic::iced::Background::Color(
                    cosmic.palette.neutral_2.into(),
                )),
                border: cosmic::iced::Border {
                    radius: cosmic.corner_radii.radius_s.into(),
                    ..Default::default()
                },
                shadow: cosmic::iced::Shadow::default(),
                snap: true,
            }
        }))
        .padding(xxs)
        .gap(1)
        .into()
}

/// The accent-tinted button styling worn by the backend Update chip and the
/// header bar's "Update available" badge.
///
/// Shared rather than copied because the two are the same control in two
/// places: both say a newer version is waiting, and both apply it. A user who
/// learns the chip on the Models page should recognize the badge in the header
/// without being told.
///
/// The translucent fill is the capability chips' so the family reads as one;
/// the accent hue is what marks this one as actionable. `disabled` repeats the
/// `active` fill because these are never rendered disabled — they are removed
/// instead — and a dimmed ghost would misreport that.
pub(in crate::ui::views) fn accent_button_class(radius: [f32; 4]) -> cosmic::theme::Button {
    let fg: cosmic::iced::Color = cosmic::theme::active().cosmic().accent.base.into();
    let style = move |alpha: f32| {
        let mut fill = fg;
        fill.a = alpha;
        let mut edge = fg;
        edge.a = 0.32;
        cosmic::widget::button::Style {
            background: Some(cosmic::iced::Background::Color(fill)),
            border_radius: radius.into(),
            border_width: 1.0,
            border_color: edge,
            icon_color: Some(fg),
            text_color: Some(fg),
            ..cosmic::widget::button::Style::new()
        }
    };
    cosmic::theme::Button::Custom {
        active: Box::new(move |_, _| style(0.14)),
        disabled: Box::new(move |_| style(0.14)),
        hovered: Box::new(move |_, _| style(0.26)),
        pressed: Box::new(move |_, _| style(0.34)),
    }
}
