// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::iced::widget::{column, row};
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::core::app::AppModel;
use crate::daemon::backends::BackendInfo;
use crate::ui::icons;
use crate::ui::messages::{Message, ModelsPageMessage};

use super::active::backend_glyph_tile;
use super::chips::{
    CloudEgress, backend_has_user_url, backend_is_online, backend_supports_cpu,
    backend_supports_gpu, capability_chip, capability_chips, models_inventory,
};
use super::surface::{
    card_divider, card_surface, card_title_block, muted_text_color, repo_button, rounded_tooltip,
};

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
    // An update this card started is the same install pipeline Browse drives,
    // so it reports on the same channel. Without reading it the card showed
    // nothing at all while an update ran, and "nothing happened" is exactly how
    // that reads to the user.
    let in_flight = app.registry.installs.get(source.as_str());
    let update_version = update_offer(registry_entry.copied(), in_flight.is_some());
    let description = registry_entry
        .and_then(|e| e.description.clone())
        .filter(|d| !d.is_empty());

    // Overflow ("⋯") menu: the optional Update, then Uninstall. Configure left
    // the menu to become a visible button beside it.
    let menu_open = app.models_page.installed_menu_open.as_deref() == Some(source.as_str());
    // Built before the menu takes ownership of the version. The Update action
    // lives behind the "⋯", which nothing on a settled card hints at; the badge
    // is what says a version is waiting, while the menu stays where it is
    // applied.
    let badge = update_version
        .as_deref()
        .map(|v| update_badge(v, !menu_open));
    let trigger = button::icon(icons::phosphor_handle(icons::DOTS_THREE_VERTICAL)).on_press(
        Message::ModelsPage(ModelsPageMessage::ToggleInstalledMenu(source.clone())),
    );
    let mut overflow = widget::popover(trigger).position(widget::popover::Position::Bottom);
    if menu_open {
        overflow = overflow
            .popup(installed_overflow_menu(&source, update_version))
            .on_close(Message::ModelsPage(ModelsPageMessage::CloseInstalledMenu));
    }

    let mut actions = widget::row::with_capacity(5)
        .spacing(spacing.space_xs)
        .align_y(Alignment::Center);
    if let Some(s) = in_flight {
        actions = actions.push(update_status(s));
    } else if let Some(b) = badge {
        actions = actions.push(b);
    }
    actions = actions
        .push(repo_button(&source))
        .push(button::standard("Configure").on_press(Message::ModelsPage(
            ModelsPageMessage::OpenBackendConfig(source.clone()),
        )))
        .push(overflow);
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
    let egress = online.then(|| CloudEgress {
        hosts: backend.allowed_hosts.as_slice(),
        user_url: backend_has_user_url(backend),
    });
    let inventory = models_inventory(&model_names);
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

/// The version an Update entry should offer, or `None` for no entry.
///
/// Compared as semver, so a stale or older index never prompts a downgrade.
/// Withheld while an install is in flight for this backend: the entry would
/// otherwise stay clickable during its own update and every further click would
/// reach a daemon that has nothing left to do.
///
/// `installed_version` is annotated onto the registry catalog, not the backends
/// list, so it goes stale unless that catalog is refetched when an install
/// settles — which is what leaves an Update entry standing after the update it
/// describes has already happened.
fn update_offer(
    entry: Option<&super_stt_shared::registry::RegistryBackend>,
    in_flight: bool,
) -> Option<String> {
    if in_flight {
        return None;
    }
    let e = entry?;
    let installed = e.installed_version.as_deref()?;
    super_stt_registry_types::version::update_available(installed, &e.version)
        .then(|| e.version.clone())
}

/// Badge marking a backend with a newer version available, carrying the version
/// in its tooltip.
///
/// Accent-colored rather than neutral: unlike the capability chips beside it,
/// this reports something the user can act on. It is not itself a button — the
/// action is the Update entry in the "⋯" menu, and two ways to start the same
/// install is one more than the card needs.
///
/// `tooltips` is off while that menu is open, for the same reason the capability
/// chips suppress theirs: a tooltip would paint half-behind the menu.
fn update_badge(version: &str, tooltips: bool) -> Element<'static, Message> {
    let accent: cosmic::iced::Color = cosmic::theme::active().cosmic().accent.base.into();
    let chip = capability_chip(icons::ARROWS_CLOCKWISE, "Update", accent);
    if tooltips {
        rounded_tooltip(
            chip,
            text::body(format!("Version {version} is available")),
            widget::tooltip::Position::Top,
        )
    } else {
        chip
    }
}

/// Progress for an update running on an installed backend, shown beside the
/// card's actions.
///
/// Reads the same `InstallStatus` a Browse install reports on — an update *is*
/// an install — so the phase and percentage mean what they mean there. It is a
/// caption rather than a button: nothing here is actionable while the daemon is
/// working, and the Update entry is withheld for the duration.
fn update_status(s: &crate::state::registry::InstallStatus) -> Element<'static, Message> {
    let muted = muted_text_color();
    let label = match (&s.error, s.bytes_total) {
        (Some(err), _) => format!("Update failed: {err}"),
        (None, Some(total)) if total > 0 => {
            format!(
                "Updating\u{2026} ({}) {}%",
                super::download::phase_label(s.phase),
                (s.bytes_done * 100) / total
            )
        }
        _ => format!(
            "Updating\u{2026} ({})",
            super::download::phase_label(s.phase)
        ),
    };
    text::caption(label)
        .class(cosmic::theme::Text::Color(muted))
        .into()
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
            Message::ModelsPage(ModelsPageMessage::UpdateBackend(source.to_string())),
        ));
    }
    col = col.push(item(
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

#[cfg(test)]
mod update_offer_tests {
    //! Pin when an Update entry is offered. The failure this guards against is
    //! not a wrong version but a stale one: `installed_version` rides on the
    //! registry catalog, so an entry can survive the update it describes and
    //! then do nothing when clicked.
    use super::update_offer;
    use super_stt_shared::registry::RegistryBackend;

    fn entry(installed: Option<&str>, latest: &str) -> RegistryBackend {
        RegistryBackend {
            id: "y".to_string(),
            source: "github.com/x/y".to_string(),
            version: latest.to_string(),
            name: "Y".to_string(),
            description: None,
            license: "Apache-2.0".to_string(),
            kind: "wasm".to_string(),
            contract: "v1".to_string(),
            allowed_hosts: Vec::new(),
            online: true,
            supports_gpu: false,
            supports_cpu: false,
            models: Vec::new(),
            secrets: Vec::new(),
            options: Vec::new(),
            compatibility: super_stt_shared::registry::Compatibility {
                compatible: true,
                selected_asset: None,
                reason: None,
            },
            installed_version: installed.map(String::from),
            index_stale: None,
        }
    }

    #[test]
    fn offers_only_a_newer_version() {
        assert_eq!(
            update_offer(Some(&entry(Some("0.1.0"), "0.1.1")), false),
            Some("0.1.1".to_string())
        );
        // Already current — this is the state that was being drawn from a stale
        // catalog and clicked repeatedly.
        assert_eq!(
            update_offer(Some(&entry(Some("0.1.1"), "0.1.1")), false),
            None
        );
        // An index older than what is installed must not prompt a downgrade.
        assert_eq!(
            update_offer(Some(&entry(Some("0.2.0"), "0.1.1")), false),
            None
        );
    }

    #[test]
    fn withholds_while_an_install_is_in_flight() {
        assert_eq!(
            update_offer(Some(&entry(Some("0.1.0"), "0.1.1")), true),
            None
        );
    }

    #[test]
    fn needs_a_catalog_entry_and_an_installed_version() {
        assert_eq!(update_offer(None, false), None);
        assert_eq!(update_offer(Some(&entry(None, "0.1.1")), false), None);
    }
}
