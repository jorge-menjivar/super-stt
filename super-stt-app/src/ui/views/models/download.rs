// SPDX-License-Identifier: GPL-3.0-only
use cosmic::iced::{Alignment, Length};
use cosmic::iced_widget::{column, row};
use cosmic::widget::{self, button, space::horizontal as horizontal_space, text};
use cosmic::{Apply, Element};

use crate::core::app::AppModel;
use crate::state::ContextPage;
use crate::ui::icons;
use crate::ui::messages::{Message, ModelsPageMessage, ShellMessage};

use super::active::backend_header;
use super::chips::{capability_chips, result_count};
use super::surface::{card_divider, card_surface, models_line, muted_text_color};

/// Browse tab split into its two regions: the fixed search + filter toolbar
/// (with the live result count) and the scrollable list of backend cards.
/// `page()` pins the toolbar above the scroll frame and scrolls only the cards.
pub(super) fn download_split(app: &AppModel) -> (Element<'_, Message>, Element<'_, Message>) {
    let spacing = cosmic::theme::spacing();

    // If the registry hasn't returned any data yet, the toolbar stands alone
    // (no count) and the scroll area carries the empty/error state.
    if app.registry.backends.is_empty() {
        return (
            download_toolbar(app, horizontal_space().into()),
            download_empty_state(app),
        );
    }

    // `total` is the candidate pool (not already installed, and compatible
    // unless the user opted in); `cards` is what survives the active
    // search / target filters. The count reads "{shown} of {total}".
    let installed_sources: std::collections::HashSet<&str> =
        app.backends.iter().map(|b| b.source.as_str()).collect();
    let filters = &app.registry.filters;

    let mut cards: Vec<Element<'_, Message>> = Vec::new();
    for entry in &app.registry.backends {
        if installed_sources.contains(entry.source.as_str()) {
            continue;
        }
        if !filters.include_incompatible && !entry.compatibility.compatible {
            continue;
        }
        if let Some(o) = filters.online
            && entry.online != o
        {
            continue;
        }
        if !filters.search.is_empty() && !registry_entry_matches(entry, &filters.search) {
            continue;
        }
        cards.push(download_card(app, entry));
    }

    let toolbar = download_toolbar(app, result_count(cards.len()));

    let list = if cards.is_empty() {
        text::body("No backends match your search. Try clearing filters or adding one manually.")
            .into()
    } else {
        // Two backends per row with a 12px (space_xs) gap. Cards are width-Fill,
        // so each pair splits the row 50/50; a lone trailing card is padded with a
        // flexible space to keep it in the left column instead of stretching full width.
        let mut grid = widget::column::with_capacity(cards.len().div_ceil(2))
            .spacing(spacing.space_xs)
            .width(Length::Fill);
        let mut iter = cards.into_iter();
        loop {
            let Some(left) = iter.next() else { break };
            let right = iter.next().unwrap_or_else(|| horizontal_space().into());
            grid = grid.push(
                row![left, right]
                    .spacing(spacing.space_xs)
                    .width(Length::Fill),
            );
        }
        grid.into()
    };
    (toolbar, list)
}

/// Search + filter toolbar for the Browse tab. Top row: a prominent search
/// field (with a built-in clear button) plus the Add-backend and Refresh
/// actions. Bottom row: the "Runs on" segmented filter, the incompatible
/// toggle, and — pushed to the right — the live result count.
pub(super) fn download_toolbar<'a>(
    app: &'a AppModel,
    count: Element<'a, Message>,
) -> Element<'a, Message> {
    use super::chips::chip_group;
    let spacing = cosmic::theme::spacing();
    let filters = &app.registry.filters;

    let search = widget::search_input(
        "Search backends, models, or providers\u{2026}",
        &filters.search,
    )
    .on_input(|x| Message::ModelsPage(ModelsPageMessage::RegistrySearchChanged(x)))
    .on_clear(Message::ModelsPage(
        ModelsPageMessage::RegistrySearchChanged(String::new()),
    ))
    .width(Length::Fill);

    let add_btn = button::suggested("+ Add backend").on_press(Message::Shell(
        ShellMessage::ToggleContextPage(ContextPage::AddBackend),
    ));
    // Refresh shows as an icon-only button (a refresh glyph) with a tooltip
    // rather than a text label. A `Standard`-classed custom button gives it the
    // surface fill + hairline border that matches the neighbouring text buttons.
    // Fixed height (`space_l`) squares it off so it lines up with "Add backend"
    // instead of shrinking to its icon.
    let btn_height = spacing.space_l;
    let refresh_btn = widget::tooltip(
        button::custom(
            icons::phosphor(icons::ARROWS_CLOCKWISE)
                .size(16)
                .apply(widget::container)
                .center_x(Length::Fixed(f32::from(btn_height)))
                .center_y(Length::Fill),
        )
        .class(cosmic::theme::Button::Standard)
        .padding(0)
        .height(Length::Fixed(f32::from(btn_height)))
        .on_press(Message::ModelsPage(ModelsPageMessage::RefreshRegistry)),
        widget::container(text::body("Refresh registry")).padding(spacing.space_xxs),
        widget::tooltip::Position::Bottom,
    );
    let search_row = row![search, add_btn, refresh_btn]
        .spacing(spacing.space_xs)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    // Low-cardinality filters as one-tap chips (active = accent-filled).
    let runs_on = chip_group(
        "Runs on",
        false,
        vec![
            (
                "All",
                filters.online.is_none(),
                Message::ModelsPage(ModelsPageMessage::RegistryOnlineFilter(None)),
            ),
            (
                "Local",
                filters.online == Some(false),
                Message::ModelsPage(ModelsPageMessage::RegistryOnlineFilter(Some(false))),
            ),
            (
                "Cloud",
                filters.online == Some(true),
                Message::ModelsPage(ModelsPageMessage::RegistryOnlineFilter(Some(true))),
            ),
        ],
    );
    let show_incompat = cosmic::widget::toggler(filters.include_incompatible)
        .label("Show incompatible".to_string())
        .spacing(spacing.space_xs)
        .on_toggle(|x| Message::ModelsPage(ModelsPageMessage::RegistryIncludeIncompatible(x)));

    // RUNS ON chips on the left; the incompatible toggle pushed to the right edge.
    let filter_row = row![runs_on, horizontal_space(), show_incompat]
        .spacing(spacing.space_m)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    // The result count gets its own short, left-aligned row sitting tight above
    // the filter chips; the search row keeps the normal gap below it.
    column![
        column![count, search_row]
            .spacing(spacing.space_xxxs)
            .width(Length::Fill),
        filter_row,
    ]
    .spacing(spacing.space_s)
    .width(Length::Fill)
    .into()
}

/// Whether a registry entry matches the search needle. Searches the backend
/// name, description, and repo id, plus the name + provider of every model it
/// serves — so "whisper", "voxtral", or "openai" find their backend even when
/// the term isn't in the display name.
pub(super) fn registry_entry_matches(
    entry: &super_stt_shared::registry::RegistryBackend,
    needle: &str,
) -> bool {
    let needle = needle.to_lowercase();
    let mut hay = format!(
        "{} {} {}",
        entry.name.to_lowercase(),
        entry
            .description
            .as_deref()
            .unwrap_or_default()
            .to_lowercase(),
        entry.source.to_lowercase(),
    );
    for m in &entry.models {
        hay.push(' ');
        hay.push_str(&m.name.to_lowercase());
        hay.push(' ');
        hay.push_str(&m.provider.to_lowercase());
    }
    hay.contains(&needle)
}

/// Empty/error state shown when no registry data is available yet.
pub(super) fn download_empty_state(app: &AppModel) -> Element<'_, Message> {
    use crate::state::registry::RefreshOutcome;
    let msg = match &app.registry.last_refresh {
        Some(RefreshOutcome::Failed(e)) => format!("Couldn't reach the registry: {e}"),
        Some(RefreshOutcome::Ok) => "No installable backends match your filters.".to_string(),
        None => "Loading registry\u{2026}".to_string(),
    };
    let updated_label = app.registry.generated_at.as_deref().map_or_else(
        || "Catalog not loaded".into(),
        |t| format!("Catalog updated {t}"),
    );
    column![
        text::body(msg),
        button::standard("Retry").on_press(Message::ModelsPage(ModelsPageMessage::RefreshRegistry)),
        widget::text(updated_label).size(10),
    ]
    .spacing(cosmic::theme::spacing().space_s)
    .into()
}

/// One installable backend from the live registry: header (name + source, no
/// action), capability chips, a description, a one-line list of served models,
/// then a divider over a footer whose left side carries a contextual note
/// (API-key requirement / install progress / "not compatible") and whose right
/// side carries the Install button.
pub(super) fn download_card<'a>(
    app: &'a AppModel,
    entry: &'a super_stt_shared::registry::RegistryBackend,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let muted = muted_text_color();
    let in_flight = app.registry.installs.get(&entry.source);
    // A request the daemon rejected before any background install started
    // (Tier 1 #15). The Install button stays enabled for a retry.
    let start_error = app.registry.install_errors.get(&entry.source);

    // Header carries no action — Install lives in the footer below.
    let mut card = widget::column::with_capacity(6)
        .spacing(spacing.space_s)
        .push(backend_header(
            entry.name.clone(),
            entry.source.clone(),
            vec![],
        ));

    let online_hosts = entry.online.then_some(entry.allowed_hosts.as_slice());
    if let Some(chips) =
        capability_chips(entry.supports_gpu, entry.supports_cpu, online_hosts, true)
    {
        card = card.push(chips);
    }

    if let Some(desc) = entry.description.as_deref().filter(|d| !d.is_empty()) {
        card = card.push(text::body(desc.to_string()).class(cosmic::theme::Text::Color(muted)));
    }

    let model_names: Vec<String> = entry.models.iter().map(|m| m.name.clone()).collect();
    if let Some(line) = models_line(&model_names) {
        card = card.push(line);
    }

    // Footer: contextual note on the left, Install button on the right.
    let install_action: Element<'a, Message> = if let Some(s) = in_flight {
        let label = match &s.error {
            Some(_) => "Failed".to_string(),
            None => format!("Installing\u{2026} ({})", phase_label(s.phase)),
        };
        button::standard(label).into()
    } else if entry.compatibility.compatible {
        button::suggested("Install")
            .on_press(Message::ModelsPage(ModelsPageMessage::InstallBackend(
                entry.source.clone(),
            )))
            .into()
    } else {
        button::standard("Not compatible").into()
    };

    let note: Element<'a, Message> = if let Some(s) = in_flight {
        match (&s.error, s.bytes_total) {
            (Some(err), _) => text::caption(format!("Failed: {err:?}"))
                .class(cosmic::theme::Text::Color(muted))
                .into(),
            (None, Some(total)) if total > 0 => {
                let pct = (s.bytes_done * 100) / total;
                text::caption(format!("{pct}%"))
                    .class(cosmic::theme::Text::Color(muted))
                    .into()
            }
            _ => horizontal_space().into(),
        }
    } else if let Some(err) = start_error {
        row![
            icons::phosphor_destructive(icons::WARNING, 14.0),
            text::caption(format!("Failed to start: {err}")),
        ]
        .spacing(spacing.space_xxs)
        .align_y(Alignment::Center)
        .into()
    } else if !entry.compatibility.compatible {
        let reason = entry
            .compatibility
            .reason
            .as_deref()
            .unwrap_or("incompatible hardware or OS");
        row![
            icons::phosphor_destructive(icons::WARNING, 14.0),
            text::caption(format!("Not compatible: {reason}")),
        ]
        .spacing(spacing.space_xxs)
        .align_y(Alignment::Center)
        .into()
    } else if entry.secrets.iter().any(|s| s.required) {
        text::caption("API key required")
            .class(cosmic::theme::Text::Color(muted))
            .into()
    } else {
        horizontal_space().into()
    };

    let footer = row![widget::container(note).width(Length::Fill), install_action,]
        .align_y(Alignment::Center);

    card = card.push(card_divider()).push(footer);

    card_surface(card, false)
}

/// Human-readable label for an [`InstallPhase`], shown inside the "Installing…"
/// button while a backend install is in progress.
pub(super) fn phase_label(p: super_stt_shared::registry::events::InstallPhase) -> &'static str {
    use super_stt_shared::registry::events::InstallPhase;
    match p {
        InstallPhase::Resolving => "resolving",
        InstallPhase::Downloading => "downloading",
        InstallPhase::Verifying => "verifying",
        InstallPhase::Extracting => "extracting",
        InstallPhase::Installing => "installing",
        InstallPhase::Rescanning => "finishing",
    }
}
