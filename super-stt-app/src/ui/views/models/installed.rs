// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::iced::{Alignment, Length};
use cosmic::iced_widget::{column, row};
use cosmic::widget::{self, button, text};

use crate::core::app::AppModel;
use crate::daemon::backends::BackendInfo;
use crate::ui::icons;
use crate::ui::messages::Message;

use super::active::backend_glyph_tile;
use super::chips::{
    backend_is_online, backend_supports_cpu, backend_supports_gpu, capability_chips,
    models_inventory,
};
use super::surface::{card_divider, card_surface, card_title_block, repo_button};

/// The Library's Installed tab: every backend the daemon discovered on disk,
/// one card each. The active backend is included too — activation lives on the
/// Models page, so the Library is a flat catalog of what's installed.
pub(super) fn installed_tab(app: &AppModel) -> Element<'_, Message> {
    let cards: Vec<Element<'_, Message>> = app
        .backends
        .iter()
        .map(|backend| installed_card(backend, app))
        .collect();

    if cards.is_empty() {
        return text::body("No backends installed. Open the Browse tab to install one.").into();
    }

    column(cards)
        .spacing(cosmic::theme::spacing().space_s)
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

    // Registry entry (by source) supplies the description shown under the name
    // and the latest version that drives the optional Update item. Compared as
    // semver so a stale/older index never prompts a downgrade.
    let registry_map = app.registry.by_source();
    let registry_entry = registry_map.get(source.as_str());
    let update_version: Option<String> = registry_entry.and_then(|e| {
        let installed = e.installed_version.as_deref()?;
        super_stt_registry_types::version::update_available(installed, &e.version)
            .then(|| e.version.clone())
    });
    let description = registry_entry
        .and_then(|e| e.description.clone())
        .filter(|d| !d.is_empty());

    // Overflow ("⋯") menu: the optional Update, then Uninstall. Configure left
    // the menu to become a visible button beside it.
    let menu_open = app.installed_menu_open.as_deref() == Some(source.as_str());
    let trigger = button::icon(icons::phosphor_handle(icons::DOTS_THREE_VERTICAL))
        .on_press(Message::ToggleInstalledMenu(source.clone()));
    let mut overflow = widget::popover(trigger).position(widget::popover::Position::Bottom);
    if menu_open {
        overflow = overflow
            .popup(installed_overflow_menu(&source, update_version))
            .on_close(Message::CloseInstalledMenu);
    }

    let actions = row![
        repo_button(&source),
        button::standard("Configure").on_press(Message::OpenBackendConfig(source.clone())),
        overflow,
    ]
    .spacing(spacing.space_xs)
    .align_y(Alignment::Center);
    let header = row![
        backend_glyph_tile(),
        card_title_block(backend.name.clone(), description),
        actions,
    ]
    .spacing(spacing.space_s)
    .align_y(Alignment::Center);

    // Facts row: the served-models inventory takes the width; the GPU / CPU /
    // Cloud capability chips sit opposite it.
    let model_names: Vec<String> = backend.models.iter().map(|m| m.name.clone()).collect();
    let hosts = online.then_some(backend.allowed_hosts.as_slice());
    let inventory = models_inventory(&model_names);
    let caps = capability_chips(
        backend_supports_gpu(backend),
        backend_supports_cpu(backend),
        hosts,
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

/// The popup body for an installed card's "⋯" overflow menu: an optional
/// Update, then Uninstall — a small rounded panel of full-width rows the
/// popover anchors below the trigger. (Configure is a visible button on the
/// card, so it's not repeated here.)
pub(super) fn installed_overflow_menu(
    source: &str,
    update_version: Option<String>,
) -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();
    let item = |label: String, msg: Message| -> Element<'static, Message> {
        button::text(label).width(Length::Fill).on_press(msg).into()
    };

    let mut col = widget::column::with_capacity(2).spacing(spacing.space_xxxs);
    if let Some(v) = update_version {
        col = col.push(item(
            format!("Update to {v}"),
            Message::UpdateBackend(source.to_string()),
        ));
    }
    col = col.push(item(
        "Uninstall".to_string(),
        Message::UninstallBackend(source.to_string()),
    ));

    widget::container(col)
        .padding(spacing.space_xxs)
        .width(Length::Fixed(190.0))
        .class(cosmic::theme::Container::custom(|theme| {
            let cosmic = theme.cosmic();
            let component = &theme.current_container().component;
            cosmic::iced_widget::container::Style {
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
