// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::iced::widget::{column, row};
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, space::horizontal as horizontal_space, text};

use crate::core::app::AppModel;
use crate::daemon::backends::BackendInfo;
use crate::state::registry::InstalledFilters;
use crate::ui::icons;
use crate::ui::messages::{Message, ModelsPageMessage};

use super::active::backend_glyph_tile;
use super::chips::{
    CloudEgress, backend_has_user_url, backend_is_online, backend_supports_cpu,
    backend_supports_gpu, capability_chips, models_inventory, role_groups, update_chip,
    update_offer, update_progress_chip,
};
use super::surface::{card_divider, card_surface, card_title_block, repo_button};

/// The Library's Installed tab: a filter toolbar over every backend the daemon
/// discovered on disk, one card each. The active backend is included too —
/// activation lives on the Models page, so the Library is a flat catalog of
/// what's installed.
pub(super) fn installed_tab(app: &AppModel) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let filters = &app.models_page.installed_filters;

    let cards: Vec<Element<'_, Message>> = app
        .backends
        .iter()
        .filter(|b| installed_matches(b, filters))
        .map(|backend| installed_card(backend, app))
        .collect();

    // "Nothing installed" and "nothing matches" are different problems with
    // different fixes, so they get different sentences.
    let body: Element<'_, Message> = if cards.is_empty() {
        if app.backends.is_empty() {
            text::body("No backends installed. Open the Browse tab to install one.").into()
        } else {
            text::body("No installed backends match these filters.").into()
        }
    } else {
        column(cards)
            .spacing(spacing.space_s)
            .width(Length::Fill)
            .into()
    };

    widget::column::with_capacity(2)
        .spacing(spacing.space_s)
        .width(Length::Fill)
        .push(installed_toolbar(filters, cards_shown(app, filters)))
        .push(body)
        .into()
}

/// Whether an installed backend survives the Installed tab's filters.
///
/// Kept free of [`AppModel`] so the rule is directly unit-testable.
fn installed_matches(backend: &BackendInfo, filters: &InstalledFilters) -> bool {
    if let Some(online) = filters.online
        && backend_is_online(backend) != online
    {
        return false;
    }
    filters
        .role
        .admits(backend.models.iter().map(|m| m.role.as_str()))
}

/// "{shown} of {total}", matching the Browse tab's count.
fn cards_shown(app: &AppModel, filters: &InstalledFilters) -> (usize, usize) {
    let shown = app
        .backends
        .iter()
        .filter(|b| installed_matches(b, filters))
        .count();
    (shown, app.backends.len())
}

/// The Installed tab's filter bar: the same "Runs on" and "Kind" chips the
/// Browse tab carries, over its own state so narrowing one list leaves the
/// other alone.
fn installed_toolbar<'a>(
    filters: &InstalledFilters,
    (shown, total): (usize, usize),
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let runs_on = super::chips::runs_on_chips(filters.online, |o| {
        Message::ModelsPage(ModelsPageMessage::InstalledOnlineFilter(o))
    });
    let kind = super::chips::role_filter_chips(filters.role, |r| {
        Message::ModelsPage(ModelsPageMessage::InstalledRoleFilter(r))
    });
    let count = text::caption(format!("{shown} of {total}")).class(cosmic::theme::Text::Color(
        super::surface::muted_text_color(),
    ));

    row![runs_on, kind, horizontal_space(), count]
        .spacing(spacing.space_m)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into()
}

/// One installed backend's Library card: the leading glyph, the name and
/// description, then a repo button + Configure + a "⋯" overflow (Update /
/// Uninstall) on the right, over a facts row of the served-models inventory
/// and capability chips. No Activate here — that's the Models page's job.
pub(super) fn installed_card<'a>(
    backend: &'a BackendInfo,
    app: &'a AppModel,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let online = backend_is_online(backend);
    let source = backend.source.clone();

    // Registry entry (by source) supplies the latest version that drives the
    // optional Update item. Compared as semver so a stale/older index never
    // prompts a downgrade.
    let registry_map = app.registry.by_source();
    let registry_entry = registry_map.get(source.as_str());
    // An update this card started is the same install pipeline Browse drives,
    // so it reports on the same channel. Without reading it the card showed
    // nothing at all while an update ran, and "nothing happened" is exactly how
    // that reads to the user.
    let in_flight = app.registry.installs.get(source.as_str());
    let update_version = update_offer(registry_entry.copied(), in_flight.is_some());
    // Registry first, installed manifest second — the same resolution the
    // Models page uses, so a sideloaded backend still describes itself here.
    let description = super::surface::backend_description(app, &source);

    // Overflow ("⋯") menu: the optional Update, then Uninstall. Configure left
    // the menu to become a visible button beside it.
    let menu_open = app.models_page.installed_menu_open.as_deref() == Some(source.as_str());
    let trigger = button::icon(icons::phosphor_handle(icons::DOTS_THREE_VERTICAL)).on_press(
        Message::ModelsPage(ModelsPageMessage::ToggleInstalledMenu(source.clone())),
    );
    let mut overflow = widget::popover(trigger).position(widget::popover::Position::Bottom);
    if menu_open {
        overflow = overflow
            .popup(installed_overflow_menu(&source))
            .on_close(Message::ModelsPage(ModelsPageMessage::CloseInstalledMenu));
    }

    let mut actions = widget::row::with_capacity(5)
        .spacing(spacing.space_xs)
        .align_y(Alignment::Center);
    if let Some(s) = in_flight {
        actions = actions.push(update_progress_chip(s));
    } else if let Some(v) = update_version.as_deref() {
        actions = actions.push(update_chip(&source, v, !menu_open));
    }
    actions = actions
        .push(repo_button(&source))
        .push(button::standard("Configure").on_press(Message::ModelsPage(
            ModelsPageMessage::OpenBackendConfig(source.clone()),
        )))
        .push(overflow);
    let header = row![
        backend_glyph_tile(),
        card_title_block(backend.name.clone(), &backend.version, description),
        actions,
    ]
    .spacing(spacing.space_s)
    .align_y(Alignment::Center);

    // Facts row: the served-models inventory — one line per kind of model the
    // backend ships — takes the width; the GPU / CPU / Cloud capability chips
    // sit opposite it.
    let groups = role_groups(
        backend
            .models
            .iter()
            .map(|m| (m.name.as_str(), m.role.as_str())),
    );
    let egress = online.then(|| CloudEgress {
        hosts: backend.allowed_hosts.as_slice(),
        user_url: backend_has_user_url(backend),
    });
    let inventory = models_inventory(&groups);
    let caps = capability_chips(
        backend_supports_gpu(backend),
        backend_supports_cpu(backend),
        egress,
        // Suppress chip tooltips while this card's overflow menu is open, so the
        // menu paints cleanly on top instead of a tooltip showing half-behind it.
        !menu_open,
    );

    let mut card = widget::column::with_capacity(3)
        .spacing(spacing.space_s)
        .push(header);
    if inventory.is_some() || caps.is_some() {
        card = card.push(card_divider());
        let mut meta = widget::row::with_capacity(2)
            .spacing(spacing.space_s)
            .align_y(Alignment::Center)
            .width(Length::Fill);
        if let Some(inv) = inventory {
            meta = meta.push(widget::container(inv).width(Length::Fill));
        }
        if let Some(c) = caps {
            meta = meta.push(c);
        }
        card = card.push(meta);
    }
    let surface = card_surface(card, false);

    // Surface a failed uninstall directly under its card until the user
    // retries (or it succeeds and the backend leaves the catalog).
    match app.registry.uninstall_errors.get(source.as_str()) {
        Some(err) => column(vec![
            surface,
            text::caption(format!("Uninstall failed: {err}")).into(),
        ])
        .spacing(cosmic::theme::spacing().space_xxxs)
        .width(Length::Fill)
        .into(),
        None => surface,
    }
}

/// The popup body for an installed card's "⋯" overflow menu: Uninstall, in a
/// small rounded panel the popover anchors below the trigger.
///
/// Only the destructive action lives here. Configure is a visible button on the
/// card and Update is the accent chip beside it, so neither is repeated — an
/// update offered in two places is one more than the card needs, and the chip
/// is the one a settled card gives a reason to look at.
pub(super) fn installed_overflow_menu(source: &str) -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();
    let item = |label: String, msg: Message| -> Element<'static, Message> {
        button::text(label).width(Length::Fill).on_press(msg).into()
    };

    let col = widget::column::with_capacity(1)
        .spacing(spacing.space_xxxs)
        .push(item(
            "Uninstall".to_string(),
            Message::ModelsPage(ModelsPageMessage::UninstallBackend(source.to_string())),
        ));

    widget::container(col)
        .padding(spacing.space_xxs)
        .width(Length::Fixed(190.0))
        .class(cosmic::theme::Container::custom(|theme| {
            let cosmic = theme.cosmic();
            let component = &theme.current_container().component;
            cosmic::iced::widget::container::Style {
                background: Some(cosmic::iced::Background::Color(component.base.into())),
                border: cosmic::iced::Border {
                    radius: cosmic.corner_radii.radius_s.into(),
                    width: 1.0,
                    color: component.divider.into(),
                },
                shadow: cosmic::iced::Shadow {
                    color: cosmic::iced::Color {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.25,
                    },
                    offset: cosmic::iced::Vector::new(0.0, 4.0),
                    blur_radius: 12.0,
                },
                snap: true,
                ..Default::default()
            }
        }))
        .into()
}
