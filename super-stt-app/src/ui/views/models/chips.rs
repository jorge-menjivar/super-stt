// SPDX-License-Identifier: GPL-3.0-only
use cosmic::iced::Alignment;
use cosmic::iced::widget::row;
use cosmic::widget::{self, text};
use cosmic::{Apply, Element};

use crate::ui::icons;
use crate::ui::messages::Message;

use super::surface::muted_text_color;

/// Whether a backend's models are served by an online provider. Online
/// backends transmit audio to a third-party service, flagged in the UI.
pub(super) fn backend_is_online(backend: &crate::daemon::backends::BackendInfo) -> bool {
    backend
        .models
        .iter()
        .any(|m| m.supported_devices.iter().any(|d| d == "none"))
}

/// Whether any model this backend serves can run on a GPU (CUDA or Metal).
/// Drives the "GPU" capability chip on the backend card.
pub(super) fn backend_supports_gpu(backend: &crate::daemon::backends::BackendInfo) -> bool {
    backend.models.iter().any(|m| {
        m.supported_devices
            .iter()
            .any(|d| d == "cuda" || d == "metal")
    })
}

/// Whether any model this backend serves can run on the CPU. Drives the
/// "CPU" capability chip on the backend card.
pub(super) fn backend_supports_cpu(backend: &crate::daemon::backends::BackendInfo) -> bool {
    backend
        .models
        .iter()
        .any(|m| m.supported_devices.iter().any(|d| d == "cpu"))
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

/// The Cloud capability chip: a [`capability_chip`] with a hover tooltip
/// listing the hosts the backend transmits audio to. Shares the GPU/CPU
/// chips' neutral tone so "runs in the cloud" reads as a plain capability,
/// not a golden/premium value judgment.
pub(super) fn cloud_chip(fg: cosmic::iced::Color, hosts: &[String]) -> Element<'static, Message> {
    use super::surface::rounded_tooltip;
    let chip = capability_chip(icons::CLOUD, "Cloud", fg);
    if hosts.is_empty() {
        return chip;
    }
    let mut popup = widget::column::with_capacity(hosts.len() + 1)
        .push(text::body("Transmits audio to:"))
        .spacing(cosmic::theme::spacing().space_xxxs);
    for host in hosts {
        popup = popup.push(text::body(format!("• {host}")));
    }
    rounded_tooltip(chip, popup, widget::tooltip::Position::Top)
}

/// The capability-chip row for a backend: GPU / CPU advertise local compute,
/// Cloud (when `online_hosts` is `Some`) flags an online backend. Returns
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
    online_hosts: Option<&[String]>,
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
    if let Some(hosts) = online_hosts {
        chips.push(if tooltips {
            cloud_chip(neutral, hosts)
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
    //! Pin the device→capability mapping behind the GPU/CPU chips: `cuda` and
    //! `metal` both count as GPU, `cpu` as CPU, and the online sentinel `none`
    //! as neither. A backend aggregates the capability across every model it
    //! serves, so one GPU model and one CPU model surface both chips.
    use super::*;
    use crate::daemon::backends::{BackendInfo, BackendModel};

    /// Build a backend whose models declare the given device lists.
    fn backend_with_devices(per_model: &[&[&str]]) -> BackendInfo {
        BackendInfo {
            source: "github.com/super-stt/test".to_string(),
            name: "Test".to_string(),
            kind: "subprocess".to_string(),
            allowed_hosts: Vec::new(),
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
}
