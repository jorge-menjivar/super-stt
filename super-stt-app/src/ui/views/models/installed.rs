// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::iced::Length;
use cosmic::iced_widget::column;
use cosmic::widget::{self, button, text};

use crate::core::app::AppModel;
use crate::daemon::backends::BackendInfo;
use crate::daemon::catalog;
use crate::ui::icons;
use crate::ui::messages::Message;

use super::active::backend_header;
use super::chips::{
    backend_is_online, backend_supports_cpu, backend_supports_gpu, capability_chips,
};
use super::surface::card_surface;

/// The active backend lives in the fixed header card, not the list.
pub(super) fn installed_tab(app: &AppModel) -> Element<'_, Message> {
    let cards: Vec<Element<'_, Message>> = app
        .backends
        .iter()
        .filter(|b| app.active_backend.as_deref() != Some(b.source.as_str()))
        .map(|backend| installed_card(backend, app))
        .collect();

    if cards.is_empty() {
        let msg = if app.active_backend.is_some() {
            "No other backends installed."
        } else {
            "No backends installed. Open the Browse tab to install one."
        };
        return text::body(msg).into();
    }

    column(cards)
        .spacing(cosmic::theme::spacing().space_s)
        .width(Length::Fill)
        .into()
}

/// One installed (non-active) backend: the name and source-id, the three
/// actions that apply at the backend level (Uninstall / Configure / Activate),
/// and an online badge when the backend transmits audio off-device.
///
/// There's deliberately no model dropdown, no Use-GPU, no Select here — model
/// choice belongs to the active-backend card up top. Activating the backend
/// moves it there, where the user picks the model. This keeps the inactive
/// list a flat catalog rather than a row of cards each pretending to be a
/// "mini" active card.
/// Whether `latest` is a strictly newer version than `installed`. Both must be
/// valid `MAJOR.MINOR.PATCH` semver; anything `semver` can't parse yields
/// `false`, so no update is offered for non-semver versions.
pub(super) fn update_available(installed: &str, latest: &str) -> bool {
    match (
        semver::Version::parse(installed),
        semver::Version::parse(latest),
    ) {
        (Ok(have), Ok(want)) => want > have,
        _ => false,
    }
}

pub(super) fn installed_card<'a>(
    backend: &'a BackendInfo,
    app: &'a AppModel,
) -> Element<'a, Message> {
    let online = backend_is_online(backend);
    let source = backend.source.clone();

    // Look up the registry entry for this backend to determine whether an
    // update is available. The registry entry carries `installed_version`
    // (the version on disk) and `version` (the latest available). Compare them
    // as semver so a newer registry version is offered but a stale/older index
    // never prompts a downgrade. Non-semver versions fall back to inequality.
    // Whether a strictly-newer version is available (drives the menu's Update
    // item). Same semver rule as before, just surfaced inside the menu.
    let registry_map = app.registry.by_source();
    let update_version: Option<String> = registry_map.get(source.as_str()).and_then(|e| {
        let installed = e.installed_version.as_deref()?;
        update_available(installed, &e.version).then(|| e.version.clone())
    });

    // Activate is the single primary action; Configure / Update / Uninstall
    // fold into a "⋯" overflow menu so each row reads as one clear call to
    // action instead of a wall of buttons.
    let menu_open = app.installed_menu_open.as_deref() == Some(source.as_str());
    let trigger = button::icon(icons::phosphor_handle(icons::DOTS_THREE_VERTICAL))
        .on_press(Message::ToggleInstalledMenu(source.clone()));
    let mut overflow = widget::popover(trigger).position(widget::popover::Position::Bottom);
    if menu_open {
        overflow = overflow
            .popup(installed_overflow_menu(&source, update_version))
            .on_close(Message::CloseInstalledMenu);
    }

    // Capability chips sit inline in the header (before Activate), keeping each
    // installed row to a single compact line.
    let hosts = online.then(|| {
        catalog::by_source(&backend.source).map_or(&[][..], |c| c.allowed_hosts.as_slice())
    });
    let mut actions: Vec<Element<'a, Message>> = Vec::new();
    if let Some(chips) = capability_chips(
        backend_supports_gpu(backend),
        backend_supports_cpu(backend),
        hosts,
    ) {
        actions.push(chips);
    }
    actions.push(
        button::suggested("Activate")
            .on_press(Message::SelectBackend(source.clone()))
            .into(),
    );
    actions.push(overflow.into());

    let card = backend_header(backend.name.clone(), backend.source.clone(), actions);
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

/// The popup body for an installed card's "⋯" overflow menu: Configure, an
/// optional Update, then Uninstall — a small rounded panel of full-width rows
/// the popover anchors below the trigger.
pub(super) fn installed_overflow_menu(
    source: &str,
    update_version: Option<String>,
) -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();
    let item = |label: String, msg: Message| -> Element<'static, Message> {
        button::text(label).width(Length::Fill).on_press(msg).into()
    };

    let mut col = widget::column::with_capacity(3).spacing(spacing.space_xxxs);
    col = col.push(item(
        "Configure".to_string(),
        Message::OpenBackendConfig(source.to_string()),
    ));
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

#[cfg(test)]
mod update_available_tests {
    //! Pin the update-button rule: offer an update only when both versions are
    //! valid semver and the registry's is strictly newer. `0.10.0` outranks
    //! `0.2.0` (the string-compare trap); anything unparseable offers nothing.
    use super::*;

    #[test]
    fn newer_registry_version_offers_update() {
        assert!(update_available("0.1.0", "0.2.0"));
        // Double-digit minor really is newer despite "0.10" < "0.2" as strings.
        assert!(update_available("0.2.0", "0.10.0"));
    }

    #[test]
    fn equal_or_older_offers_nothing() {
        assert!(!update_available("1.2.3", "1.2.3"));
        assert!(!update_available("2.0.0", "1.9.9"));
    }

    #[test]
    fn non_semver_offers_nothing() {
        assert!(!update_available("1.2", "1.3.0")); // partial installed version
        assert!(!update_available("1.0.0", "nightly")); // non-semver registry
        assert!(!update_available("", "1.0.0"));
    }
}
