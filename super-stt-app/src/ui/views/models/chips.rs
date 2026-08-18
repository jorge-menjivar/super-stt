// SPDX-License-Identifier: GPL-3.0-only
use cosmic::iced::Alignment;
use cosmic::iced::widget::row;
use cosmic::widget::{self, text};
use cosmic::{Apply, Element};

use crate::ui::icons;
use crate::ui::messages::Message;

use super::surface::muted_text_color;

/// The version an update should offer, or `None` for no update.
///
/// Whether an update exists is the daemon's answer, read from
/// `update_available`: it is the side that reads the installed manifest off
/// disk and owns the index, so the comparison lives there and no client
/// re-derives it. This only decides whether to *show* that answer.
///
/// Withheld while an install is in flight for this backend: the chip would
/// otherwise stay clickable during its own update, and every further click
/// would reach a daemon that has nothing left to do.
///
/// The flag rides on the registry catalog, not the backends list, so it goes
/// stale unless that catalog is refetched — which is what left an update
/// offered after the update it describes had already happened.
pub(super) fn update_offer(
    entry: Option<&super_stt_shared::registry::RegistryBackend>,
    in_flight: bool,
) -> Option<String> {
    if in_flight {
        return None;
    }
    let e = entry?;
    e.update_available.then(|| e.version.clone())
}

/// Accent chip marking a backend with a newer version — and the control that
/// applies it.
///
/// Shaped like the capability chips beside it but accent-colored and clickable,
/// because unlike them it reports something the user can act on. Being the
/// action as well as the sign is what lets it work on the Models page, whose
/// card carries no other route to an update.
///
/// `tooltips` is off while a card's overflow menu is open, for the same reason
/// the capability chips suppress theirs: a tooltip would paint half-behind the
/// menu.
pub(super) fn update_chip(
    source: &str,
    version: &str,
    tooltips: bool,
) -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();
    let radius = cosmic::theme::active().cosmic().corner_radii.radius_xl;
    let fg: cosmic::iced::Color = cosmic::theme::active().cosmic().accent.base.into();
    // Same translucent fill the capability chips use, so the row reads as one
    // family; the accent hue is what sets this one apart.
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

    let chip = widget::button::custom(
        row![
            icons::phosphor_tinted(icons::ARROWS_CLOCKWISE, 14.0, fg),
            text::caption("Update").class(cosmic::theme::Text::Color(fg)),
        ]
        .spacing(spacing.space_xxxs)
        .align_y(Alignment::Center),
    )
    .padding([spacing.space_xxxs, spacing.space_xs])
    .class(cosmic::theme::Button::Custom {
        active: Box::new(move |_, _| style(0.14)),
        disabled: Box::new(move |_| style(0.14)),
        hovered: Box::new(move |_, _| style(0.26)),
        pressed: Box::new(move |_, _| style(0.34)),
    })
    .on_press(Message::ModelsPage(
        crate::ui::messages::ModelsPageMessage::UpdateBackend(source.to_string()),
    ))
    .into();

    if tooltips {
        super::surface::rounded_tooltip(
            chip,
            text::body(format!("Update to {version}")),
            widget::tooltip::Position::Top,
        )
    } else {
        chip
    }
}

/// The update chip's in-flight form: same shape, muted, and inert.
///
/// Reads the `InstallStatus` a Browse install reports on — an update *is* an
/// install — so the phase and percentage mean what they mean there. It replaces
/// the update chip in place on both cards, so the control the user pressed
/// becomes the progress they are waiting on rather than disappearing.
pub(super) fn update_progress_chip(
    s: &crate::state::registry::InstallStatus,
) -> Element<'static, Message> {
    let label = match (&s.error, s.bytes_total) {
        (Some(_), _) => "Update failed".to_string(),
        (None, Some(total)) if total > 0 => {
            format!("Updating\u{2026} {}%", (s.bytes_done * 100) / total)
        }
        _ => format!(
            "Updating\u{2026} ({})",
            super::download::phase_label(s.phase)
        ),
    };
    let fg = muted_text_color();
    let chip = inert_chip(icons::ARROWS_CLOCKWISE, label, fg);
    match &s.error {
        // The reason is too long for the chip and too important to drop.
        Some(err) => super::surface::rounded_tooltip(
            chip,
            text::body(format!("{err}")),
            widget::tooltip::Position::Top,
        ),
        None => chip,
    }
}

/// Whether a backend's models are served by an online provider. Online
/// backends transmit audio to a third-party service, flagged in the UI.
pub(super) fn backend_is_online(backend: &crate::daemon::backends::BackendInfo) -> bool {
    backend
        .models
        .iter()
        .any(|m| m.supported_devices.iter().any(|d| d == "none"))
}

/// Whether this backend's *installed build* can run models on a GPU. Drives
/// the "GPU" capability chip on the backend card.
///
/// Reads `installed_accel` — the accel of the asset actually on disk — rather
/// than the manifest alone: a CUDA-only backend installed on an AMD host
/// lands on its CPU asset, and the chip must not claim a capability that
/// asset does not have. An empty `installed_accel` means no record (a
/// local-directory import, or an install predating it), so this falls back
/// to the models' declared `supported_devices`.
pub(super) fn backend_supports_gpu(backend: &crate::daemon::backends::BackendInfo) -> bool {
    if !backend.installed_accel.is_empty() {
        return backend.installed_accel.iter().any(|a| a != "cpu");
    }
    backend.models.iter().any(|m| {
        m.supported_devices
            .iter()
            .any(|d| d == "cuda" || d == "metal" || d == "gpu")
    })
}

/// The devices a model may actually be loaded onto on this machine.
///
/// A model's `supported_devices` says what the *model* can do; the backend's
/// `installed_accel` says what the *installed build* can do. Only the
/// intersection is offerable — a CUDA-only backend on a host with no NVIDIA
/// GPU installs its CPU asset, and offering a GPU there is the defect this
/// closes.
///
/// An empty `installed_accel` means the daemon has no record — a
/// local-directory import, or an install predating the record — and the
/// manifest is then the only available answer. Online models
/// (`supported_devices == ["none"]`) offer nothing: there is no local compute.
pub(super) fn offered_devices(
    backend: &crate::daemon::backends::BackendInfo,
    model: &str,
) -> Vec<String> {
    let Some(model) = backend.models.iter().find(|m| m.name == model) else {
        return Vec::new();
    };
    if model.supported_devices.iter().any(|d| d == "none") {
        return Vec::new();
    }
    // `cuda`/`metal` are deprecated manifest spellings of `gpu`; a daemon
    // normalizes them, but an older one may not have.
    let declared: Vec<String> = model
        .supported_devices
        .iter()
        .map(|d| match d.as_str() {
            "cuda" | "metal" => "gpu".to_string(),
            other => other.to_string(),
        })
        .collect();
    if backend.installed_accel.is_empty() {
        return declared;
    }
    let accelerated = backend.installed_accel.iter().any(|a| a != "cpu");
    declared
        .into_iter()
        .filter(|d| d == "cpu" || accelerated)
        .collect()
}

/// Whether `model` is the online sentinel (`supported_devices == ["none"]`)
/// — genuinely no local device to pick, ever.
///
/// Sibling to [`offered_devices`], which also reports an empty list for this
/// case — but an empty list means two different things: this one (nothing to
/// pick because there is nothing local to run), or a local model this
/// specific install cannot run on *any* device (e.g. a GPU-only model with
/// only a CPU asset installed). A caller deciding whether to enable a "Load"
/// action must not conflate the two: only this one needs no device at all.
/// Returns `false` for an unknown model, same as `offered_devices`.
pub(super) fn model_is_online(backend: &crate::daemon::backends::BackendInfo, model: &str) -> bool {
    backend
        .models
        .iter()
        .find(|m| m.name == model)
        .is_some_and(|m| m.supported_devices.iter().any(|d| d == "none"))
}

/// Whether any model this backend serves can run on the CPU. Drives the
/// "CPU" capability chip on the backend card.
pub(super) fn backend_supports_cpu(backend: &crate::daemon::backends::BackendInfo) -> bool {
    backend
        .models
        .iter()
        .any(|m| m.supported_devices.iter().any(|d| d == "cpu"))
}

/// Whether the user pointed this backend at an endpoint of their own — a
/// `base_url` option carrying a value. That value is egress the backend's
/// `allowed_hosts` does not describe, so the Cloud chip has to account for it;
/// the address itself stays out of the card (see [`cloud_chip`]).
///
/// Keyed on the value being present, which is the daemon's own rule: it
/// authorizes whatever the override holds, whether or not that happens to equal
/// a `default` some older daemon still reports. Testing against `default` would
/// hide the line for a user who set the endpoint to the same string, on a card
/// whose whole job is disclosing where audio goes.
pub(super) fn backend_has_user_url(backend: &crate::daemon::backends::BackendInfo) -> bool {
    backend.options.iter().any(|o| {
        o.name == super_stt_registry_types::manifest::BASE_URL_OPTION
            && o.value.as_ref().is_some_and(|v| !v.trim().is_empty())
    })
}

/// A small rounded "pill" advertising one backend capability — a tinted icon
/// and a short label over a soft, same-hue fill. `fg` is the full-strength
/// tone (icon, text, and border); the fill and border are derived from it at a
/// lower alpha so the chip reads as a tag, not a button.
pub(super) fn capability_chip(
    icon: &'static [u8],
    label: &'static str,
    fg: cosmic::iced::Color,
) -> Element<'static, Message> {
    inert_chip(icon, label.to_string(), fg)
}

/// The chip shape itself, for a label only known at runtime. `capability_chip`
/// is the fixed-label form; both render identically so a chip built from a
/// progress percentage sits in a row of capability chips without looking
/// foreign.
pub(super) fn inert_chip(
    icon: &'static [u8],
    label: String,
    fg: cosmic::iced::Color,
) -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();
    let radius = cosmic::theme::active().cosmic().corner_radii.radius_xl;
    let mut fill = fg;
    fill.a = 0.14;
    let mut edge = fg;
    edge.a = 0.32;

    row![
        icons::phosphor_tinted(icon, 14.0, fg),
        text::caption(label).class(cosmic::theme::Text::Color(fg)),
    ]
    .spacing(spacing.space_xxxs)
    .align_y(Alignment::Center)
    .apply(widget::container)
    .padding([spacing.space_xxxs, spacing.space_xs])
    .class(cosmic::theme::Container::custom(move |_| {
        cosmic::iced::widget::container::Style {
            background: Some(cosmic::iced::Background::Color(fill)),
            border: cosmic::iced::Border {
                radius: radius.into(),
                width: 1.0,
                color: edge,
            },
            ..Default::default()
        }
    }))
    .into()
}

/// A neutral, text-only pill — same shape/tone as [`capability_chip`] but
/// without a leading glyph. Used for the active card's "N models" count.
pub(super) fn count_chip(label: String) -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();
    let radius = cosmic::theme::active().cosmic().corner_radii.radius_xl;
    let fg: cosmic::iced::Color = cosmic::theme::active()
        .current_container()
        .component
        .on
        .into();
    let mut fill = fg;
    fill.a = 0.14;
    let mut edge = fg;
    edge.a = 0.32;

    text::caption(label)
        .class(cosmic::theme::Text::Color(fg))
        .apply(widget::container)
        .padding([spacing.space_xxxs, spacing.space_xs])
        .class(cosmic::theme::Container::custom(move |_| {
            cosmic::iced::widget::container::Style {
                background: Some(cosmic::iced::Background::Color(fill)),
                border: cosmic::iced::Border {
                    radius: radius.into(),
                    width: 1.0,
                    color: edge,
                },
                ..Default::default()
            }
        }))
        .into()
}

/// How many model names a card lists individually before the rest collapse
/// into a "+N" summary chip. Three keeps the inventory to a single line for
/// typical model-name lengths.
const MAX_MODEL_TAGS: usize = 3;

/// A quiet outline "tag" for one model name: hairline border, no fill, slightly
/// dimmed text, with the gentle `radius_s` corner so it reads as a catalog item
/// rather than a status pill (which the capability chips own, fully rounded).
pub(super) fn model_tag(name: String) -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();
    let radius = cosmic::theme::active().cosmic().corner_radii.radius_s;
    let on: cosmic::iced::Color = cosmic::theme::active()
        .current_container()
        .component
        .on
        .into();
    let mut edge = on;
    edge.a = 0.28;
    let mut fg = on;
    fg.a = 0.85;

    text::caption(name)
        .class(cosmic::theme::Text::Color(fg))
        .apply(widget::container)
        .padding([spacing.space_xxxs, spacing.space_xs])
        .class(cosmic::theme::Container::custom(move |_| {
            cosmic::iced::widget::container::Style {
                border: cosmic::iced::Border {
                    radius: radius.into(),
                    width: 1.0,
                    color: edge,
                },
                ..Default::default()
            }
        }))
        .into()
}

/// A backend's model inventory: a muted "Models" label, the first
/// [`MAX_MODEL_TAGS`] model names as outline [`model_tag`]s, then a filled
/// "+N" [`count_chip`] summarizing the rest. `None` when the backend serves no
/// models, so the caller skips the row.
pub(super) fn models_inventory(names: &[String]) -> Option<Element<'static, Message>> {
    if names.is_empty() {
        return None;
    }
    let spacing = cosmic::theme::spacing();
    let muted = muted_text_color();
    let mut inventory = row![text::caption("Models").class(cosmic::theme::Text::Color(muted))]
        .spacing(spacing.space_xxs)
        .align_y(Alignment::Center);
    for name in names.iter().take(MAX_MODEL_TAGS) {
        inventory = inventory.push(model_tag(name.clone()));
    }
    let rest = names.len().saturating_sub(MAX_MODEL_TAGS);
    if rest > 0 {
        inventory = inventory.push(count_chip(format!("+{rest}")));
    }
    Some(inventory.into())
}

/// Where an online backend sends audio, as a card can describe it: the hosts
/// its `backend.toml` declares, plus whether the user pointed it at an endpoint
/// of their own.
#[derive(Clone, Copy)]
pub(super) struct CloudEgress<'a> {
    /// The backend's declared `[network].allowed_hosts`.
    pub hosts: &'a [String],
    /// Whether a `base_url` the user set adds an endpoint beyond `hosts`.
    pub user_url: bool,
}

/// The Cloud capability chip: a [`capability_chip`] with a hover tooltip
/// listing the hosts the backend transmits audio to. Shares the GPU/CPU
/// chips' neutral tone so "runs in the cloud" reads as a plain capability,
/// not a golden/premium value judgment.
///
/// A user-set `base_url` is named as a line, never as the address: it is the
/// user's own value, and printing it would put a configured endpoint on a card
/// they may be showing someone.
pub(super) fn cloud_chip(
    fg: cosmic::iced::Color,
    egress: CloudEgress<'_>,
) -> Element<'static, Message> {
    use super::surface::rounded_tooltip;
    let chip = capability_chip(icons::CLOUD, "Cloud", fg);
    if egress.hosts.is_empty() && !egress.user_url {
        return chip;
    }
    let mut popup = widget::column::with_capacity(egress.hosts.len() + 2)
        .push(text::body("Transmits audio to:"))
        .spacing(cosmic::theme::spacing().space_xxxs);
    for host in egress.hosts {
        popup = popup.push(text::body(format!("• {host}")));
    }
    if egress.user_url {
        popup = popup.push(text::body("• another URL you set"));
    }
    rounded_tooltip(chip, popup, widget::tooltip::Position::Top)
}

/// The capability-chip row for a backend: GPU / CPU advertise local compute,
/// Cloud (when `online` is `Some`) flags an online backend. Returns
/// `None` when there's nothing to advertise, so callers skip the row rather
/// than render an empty band.
///
/// `tooltips` gates the hover popovers (GPU/CPU detail, Cloud host list). The
/// Library installed card passes `!menu_open`: while that card's "⋯" overflow
/// menu is open, its chips drop their tooltips so the menu renders cleanly on
/// top (libcosmic draws the open menu above a tooltip, so a tooltip showing at
/// the same time would paint half-behind it). With the menu closed — and on
/// the active-backend / Browse cards, which have no overflow menu — tooltips
/// show as normal.
// reason: "supports_gpu" / "supports_cpu" are the clearest names
#[allow(clippy::similar_names)]
pub(super) fn capability_chips(
    supports_gpu: bool,
    supports_cpu: bool,
    online: Option<CloudEgress<'_>>,
    tooltips: bool,
) -> Option<Element<'static, Message>> {
    use super::surface::rounded_tooltip;
    let theme = cosmic::theme::active();
    let neutral: cosmic::iced::Color = theme.current_container().component.on.into();

    let mut chips: Vec<Element<'static, Message>> = Vec::new();
    if supports_gpu {
        let chip = capability_chip(icons::GRAPHICS_CARD, "GPU", neutral);
        chips.push(if tooltips {
            rounded_tooltip(
                chip,
                text::body("Accelerated on GPU"),
                widget::tooltip::Position::Top,
            )
        } else {
            chip
        });
    }
    if supports_cpu {
        let chip = capability_chip(icons::CPU, "CPU", neutral);
        chips.push(if tooltips {
            rounded_tooltip(
                chip,
                text::body("Runs on the CPU"),
                widget::tooltip::Position::Top,
            )
        } else {
            chip
        });
    }
    if let Some(egress) = online {
        chips.push(if tooltips {
            cloud_chip(neutral, egress)
        } else {
            capability_chip(icons::CLOUD, "Cloud", neutral)
        });
    }
    if chips.is_empty() {
        return None;
    }
    Some(
        row(chips)
            .spacing(cosmic::theme::spacing().space_xxs)
            .align_y(Alignment::Center)
            .into(),
    )
}

/// A segmented control of mutually-exclusive filter options: a caption label
/// followed by chips butted together inside a single rounded "track", so the
/// group reads as one unified toggle rather than separate buttons. The active
/// chip is filled — accent by default, or a neutral surface when `neutral` is
/// set (used for the secondary "Format" filter); inactive chips are transparent
/// so the track shows through.
pub(super) fn chip_group(
    label: &str,
    neutral: bool,
    chips: Vec<(&'static str, bool, Message)>,
) -> Element<'static, Message> {
    use cosmic::widget::button;
    let spacing = cosmic::theme::spacing();
    let muted = muted_text_color();

    // Chips with no gap between them; the surrounding track supplies the inset.
    let mut segments = row![].spacing(0).align_y(Alignment::Center);
    for (chip_label, active, msg) in chips {
        let chip = if active {
            if neutral {
                button::standard(chip_label)
            } else {
                button::suggested(chip_label)
            }
        } else {
            button::text(chip_label)
        }
        // Match the font size (14) so the label's line box hugs the glyph and
        // sits vertically centered, rather than floating high in the stock 20px
        // line box. Same centering technique `pill_label` uses (`line_height(1.0)`
        // = 1.0 × 14px); these stay regular buttons, not pills.
        .line_height(14)
        .padding([spacing.space_xxs, spacing.space_s])
        .on_press(msg);
        segments = segments.push(chip);
    }

    // The track: a surface-filled, hairline-bordered, pill-rounded container
    // with a small inset so the active chip visually sits within it.
    let track = widget::container(segments)
        .padding(3)
        .class(cosmic::theme::Container::custom(
            super::surface::pill_surface,
        ));

    row![
        text::caption(label.to_uppercase()).class(cosmic::theme::Text::Color(muted)),
        track
    ]
    .spacing(spacing.space_xs)
    .align_y(Alignment::Center)
    .into()
}

/// The "{shown} backends found" result-count caption above the filter chips.
pub(super) fn result_count<'a>(shown: usize) -> Element<'a, Message> {
    let muted = muted_text_color();
    let label = format!("{shown} backends found");
    text::caption(label)
        .class(cosmic::theme::Text::Color(muted))
        .into()
}

/// One unmet-requirement row: a destructive-colored warning glyph and the
/// message `"{label} must be set."`. The active-backend card is the obvious
/// source of the constraint, so the message stays short — no backend name,
/// no internal identifier. Only the icon is tinted red; the text uses the
/// default body color so the row reads cleanly. Non-dismissible: this row
/// disappears the moment the requirement is satisfied (no click required).
pub(super) fn requirement_warning(label: &str) -> Element<'_, Message> {
    row![
        icons::phosphor_destructive(icons::WARNING, 16.0),
        text::body(format!("{label} must be set.")),
    ]
    .spacing(cosmic::theme::spacing().space_xs)
    .align_y(Alignment::Center)
    .into()
}

#[cfg(test)]
mod capability_tests {
    //! Pin the device→capability mapping behind the GPU/CPU chips and the
    //! device picker. GPU capability is a property of what the *install* can
    //! do: `installed_accel` is authoritative when present (a non-`cpu` entry
    //! means GPU-capable), falling back to the manifest's `supported_devices`
    //! (`cuda`/`metal`/`gpu` count as GPU, `cpu` as CPU) when there is no
    //! install record. The online sentinel `none` counts as neither. A
    //! backend aggregates capability across every model it serves, so one GPU
    //! model and one CPU model surface both chips.
    use super::*;
    use crate::daemon::backends::{BackendInfo, BackendModel};

    /// Build a backend whose models declare the given device lists.
    fn backend_with_devices(per_model: &[&[&str]]) -> BackendInfo {
        BackendInfo {
            source: "github.com/super-stt/test".to_string(),
            name: "Test".to_string(),
            version: "1.0.0".to_string(),
            kind: "subprocess".to_string(),
            allowed_hosts: Vec::new(),
            installed_accel: Vec::new(),
            models: per_model
                .iter()
                .enumerate()
                .map(|(i, devices)| BackendModel {
                    name: format!("m{i}"),
                    provider: String::new(),
                    supported_devices: devices.iter().map(|s| (*s).to_string()).collect(),
                    estimated_vram_bytes: 0,
                    multilingual: false,
                    supported_languages: Vec::new(),
                    primary_language: String::new(),
                    realtime: false,
                })
                .collect(),
            secrets: Vec::new(),
            options: Vec::new(),
        }
    }

    /// Both GPU backends (`cuda`, `metal`) surface the GPU chip, including
    /// when paired with `cpu` in the same model's device list.
    #[test]
    fn cuda_and_metal_count_as_gpu() {
        assert!(backend_supports_gpu(&backend_with_devices(&[&["cuda"]])));
        assert!(backend_supports_gpu(&backend_with_devices(&[&["metal"]])));
        assert!(backend_supports_gpu(&backend_with_devices(&[&[
            "cpu", "cuda"
        ]])));
    }

    /// A `cpu`-only model is CPU-capable and not GPU-capable.
    #[test]
    fn cpu_only_is_cpu_not_gpu() {
        let b = backend_with_devices(&[&["cpu"]]);
        assert!(backend_supports_cpu(&b));
        assert!(!backend_supports_gpu(&b));
    }

    /// The online sentinel `none` advertises no local compute at all — its
    /// card shows the Cloud chip instead (driven separately by online-ness).
    #[test]
    fn online_sentinel_is_neither() {
        let b = backend_with_devices(&[&["none"]]);
        assert!(!backend_supports_gpu(&b));
        assert!(!backend_supports_cpu(&b));
    }

    /// Capability is the union across a backend's models: a CPU-only model
    /// plus a GPU-only model yields both chips.
    #[test]
    fn capability_aggregates_across_models() {
        let b = backend_with_devices(&[&["cpu"], &["cuda"]]);
        assert!(backend_supports_gpu(&b));
        assert!(backend_supports_cpu(&b));
    }

    /// A one-model backend whose model declares `supported` and whose install
    /// record declares `installed`.
    fn backend_with_install(supported: &[&str], installed: &[&str]) -> BackendInfo {
        let mut backend = backend_with_devices(&[supported]);
        backend.models[0].name = "m".to_string();
        backend.installed_accel = installed.iter().map(|a| (*a).to_string()).collect();
        backend
    }

    /// The reported defect: a CUDA-only backend on a host without an NVIDIA
    /// GPU installs its CPU asset, and the picker must then offer the CPU
    /// alone. Reading `supported_devices` on its own offers a GPU that cannot
    /// be used.
    #[test]
    fn a_cpu_install_offers_only_the_cpu() {
        let backend = backend_with_install(&["cpu", "gpu"], &["cpu"]);
        assert_eq!(offered_devices(&backend, "m"), vec!["cpu".to_string()]);
    }

    #[test]
    fn an_accelerated_install_offers_both() {
        for accel in [["cuda"], ["rocm"], ["vulkan"]] {
            let backend = backend_with_install(&["cpu", "gpu"], &accel);
            assert_eq!(
                offered_devices(&backend, "m"),
                vec!["cpu".to_string(), "gpu".to_string()],
                "install {accel:?} must offer both"
            );
        }
    }

    /// A model that declares no GPU path stays CPU-only even on an
    /// accelerated install — one asset serves several models.
    #[test]
    fn a_cpu_only_model_stays_cpu_on_an_accelerated_install() {
        let backend = backend_with_install(&["cpu"], &["cuda"]);
        assert_eq!(offered_devices(&backend, "m"), vec!["cpu".to_string()]);
    }

    /// No record — a local-directory import, or an install predating it.
    /// Falling back to the manifest is the only answer available.
    #[test]
    fn an_unrecorded_install_falls_back_to_the_manifest() {
        let backend = backend_with_install(&["cpu", "gpu"], &[]);
        assert_eq!(
            offered_devices(&backend, "m"),
            vec!["cpu".to_string(), "gpu".to_string()]
        );
    }

    /// A daemon older than the `gpu` vocabulary still says `cuda`; the app
    /// normalizes rather than showing a device the picker cannot stage.
    #[test]
    fn a_legacy_manifest_spelling_is_normalized() {
        let backend = backend_with_install(&["cpu", "cuda"], &["cuda"]);
        assert_eq!(
            offered_devices(&backend, "m"),
            vec!["cpu".to_string(), "gpu".to_string()]
        );
    }

    #[test]
    fn an_online_model_offers_nothing_to_pick() {
        let backend = backend_with_install(&["none"], &[]);
        assert!(offered_devices(&backend, "m").is_empty());
    }

    #[test]
    fn an_unknown_model_offers_nothing() {
        let backend = backend_with_install(&["cpu", "gpu"], &["cuda"]);
        assert!(offered_devices(&backend, "absent").is_empty());
    }

    /// `offered_devices` returns empty for two different reasons, and a
    /// caller deciding whether to enable a "Load" action must tell them
    /// apart: the online sentinel needs no device at all, while a GPU-only
    /// model on an install that resolved to CPU-only can be loaded nowhere.
    /// Pinning both against the same empty-list outcome is what keeps them
    /// from silently collapsing back into one case.
    #[test]
    fn online_and_unrunnable_both_offer_nothing_but_differ_on_is_online() {
        let online = backend_with_install(&["none"], &[]);
        assert!(offered_devices(&online, "m").is_empty());
        assert!(model_is_online(&online, "m"));

        let gpu_only_on_a_cpu_install = backend_with_install(&["gpu"], &["cpu"]);
        assert!(offered_devices(&gpu_only_on_a_cpu_install, "m").is_empty());
        assert!(!model_is_online(&gpu_only_on_a_cpu_install, "m"));
    }

    #[test]
    fn model_is_online_is_false_for_a_local_model_and_an_unknown_one() {
        let backend = backend_with_install(&["cpu", "gpu"], &["cuda"]);
        assert!(!model_is_online(&backend, "m"));
        assert!(!model_is_online(&backend, "absent"));
    }

    /// The install record is authoritative over the manifest: a CUDA-labeled
    /// model on a CPU-only install shows no GPU chip — the same defect
    /// `offered_devices` closes, but for the capability chip rather than the
    /// device picker.
    #[test]
    fn a_cpu_only_install_hides_the_gpu_chip_even_if_the_manifest_claims_cuda() {
        let backend = backend_with_install(&["cpu", "cuda"], &["cpu"]);
        assert!(!backend_supports_gpu(&backend));
    }

    /// An accelerated install record is authoritative even when the model
    /// uses the unified `"gpu"` spelling instead of a legacy accel name.
    #[test]
    fn an_accelerated_install_shows_the_gpu_chip() {
        let backend = backend_with_install(&["cpu", "gpu"], &["rocm"]);
        assert!(backend_supports_gpu(&backend));
    }

    /// Build a backend declaring a `base_url` option with the given effective
    /// value — what `GET /backends` reports once the user has (or hasn't) set
    /// one.
    fn backend_with_base_url(value: Option<&str>) -> BackendInfo {
        use crate::daemon::backends::BackendOption;
        let mut b = backend_with_devices(&[&["none"]]);
        b.options = vec![BackendOption {
            name: "base_url".to_string(),
            label: None,
            description: String::new(),
            r#type: Some("string".to_string()),
            default: None,
            required: false,
            value: value.map(ToString::to_string),
        }];
        b
    }

    /// The Cloud chip has to account for egress the manifest does not describe.
    /// A `base_url` the user set is exactly that; an unset or blank one is not,
    /// and must not put a phantom line on the card.
    #[test]
    fn user_url_is_flagged_only_once_a_value_is_set() {
        assert!(backend_has_user_url(&backend_with_base_url(Some(
            "https://gw.example.com"
        ))));
        assert!(!backend_has_user_url(&backend_with_base_url(None)));
        assert!(!backend_has_user_url(&backend_with_base_url(Some("  "))));
        // A backend that declares no such option never flags one.
        assert!(!backend_has_user_url(&backend_with_devices(&[&["none"]])));
    }

    /// The daemon authorizes whatever the override holds, so a value equal to a
    /// `default` an older daemon still reports is still egress the manifest did
    /// not declare. The card must say so rather than compare the two.
    #[test]
    fn user_url_is_flagged_even_when_it_equals_a_reported_default() {
        let mut b = backend_with_base_url(Some("https://api.example.com"));
        b.options[0].default = Some("https://api.example.com".to_string());
        assert!(backend_has_user_url(&b));
    }
}

#[cfg(test)]
mod update_offer_tests {
    //! Pin when the daemon's answer is shown. The comparison itself is the
    //! daemon's — these fix what the card does with it, including the failure
    //! this started from: the flag rides on the registry catalog, so a chip can
    //! survive the update it describes and then do nothing when clicked.
    use super::update_offer;
    use super_stt_shared::registry::RegistryBackend;

    fn entry(installed: Option<&str>, latest: &str) -> RegistryBackend {
        // Mirrors what the daemon computes, so the fixture cannot claim an
        // update the daemon would not report.
        let update_available = installed
            .is_some_and(|i| super_stt_registry_types::version::update_available(i, latest));
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
            update_available,
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
