// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;
use cosmic::iced::Length;
use cosmic::widget;

use crate::ui::messages::Message;

/// Format a byte count as a one-decimal GiB string (e.g. `"24.0 GiB"`).
// reason: display-only; the imprecision is cosmetic
#[allow(clippy::cast_precision_loss)]
pub(super) fn fmt_gib(bytes: u64) -> String {
    let gib = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    format!("{gib:.1} GiB")
}

/// Two byte counts as a shared-unit GiB pair, e.g. `"3.2 / 24.0 GiB"` — the
/// header's live "used / total" GPU memory readout, refreshed by polling.
// reason: display-only; the imprecision is cosmetic
#[allow(clippy::cast_precision_loss)]
pub(super) fn fmt_gib_pair(used: u64, total: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    format!("{:.1} / {:.1} GiB", used as f64 / GIB, total as f64 / GIB)
}

/// Strip the marketing vendor prefix from a GPU name for compact display
/// (e.g. `"NVIDIA GeForce RTX 3090"` → `"RTX 3090"`). The full name still shows
/// in the header tooltip. Display-only.
pub(super) fn short_gpu_name(name: &str) -> &str {
    const PREFIXES: &[&str] = &[
        "NVIDIA GeForce ",
        "NVIDIA ",
        "AMD Radeon ",
        "AMD ",
        "Intel Arc ",
        "Intel ",
    ];
    for p in PREFIXES {
        if let Some(rest) = name.strip_prefix(p) {
            return rest;
        }
    }
    name
}

/// A compact fixed-width used/total memory meter for the header GPU readout: a
/// neutral rounded track with an accent fill whose width tracks utilization.
/// The fill shifts success → warning → destructive as the card fills up, so
/// memory pressure reads at a glance without needing the tooltip.
// reason: display-only; the imprecision is cosmetic
#[allow(clippy::cast_precision_loss)]
pub(super) fn vram_meter<'a>(used: u64, total: u64) -> Element<'a, Message> {
    let frac = if total == 0 {
        0.0
    } else {
        (used as f32 / total as f32).clamp(0.0, 1.0)
    };
    let track_w = 72.0_f32;
    let height = 6.0_f32;
    let fill_w = (track_w * frac).clamp(2.0, track_w);

    let fill = widget::container(widget::text(""))
        .width(Length::Fixed(fill_w))
        .height(Length::Fixed(height))
        .class(cosmic::theme::Container::custom(move |theme| {
            let cosmic = theme.cosmic();
            let color: cosmic::iced::Color = if frac >= 0.9 {
                cosmic.destructive.base.into()
            } else if frac >= 0.75 {
                cosmic.warning.base.into()
            } else {
                cosmic.success.base.into()
            };
            cosmic::iced_widget::container::Style {
                background: Some(cosmic::iced::Background::Color(color)),
                border: cosmic::iced::Border {
                    radius: (height / 2.0).into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }));

    widget::container(fill)
        .width(Length::Fixed(track_w))
        .height(Length::Fixed(height))
        .class(cosmic::theme::Container::custom(move |theme| {
            let mut bg: cosmic::iced::Color = theme.cosmic().palette.neutral_5.into();
            bg.a = 0.25;
            cosmic::iced_widget::container::Style {
                background: Some(cosmic::iced::Background::Color(bg)),
                border: cosmic::iced::Border {
                    radius: (height / 2.0).into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }))
        .into()
}

/// The advisory "may not fit" row shown under the staged picker: a yellow
/// warning glyph plus the model's estimated VRAM against what the GPU has.
/// Non-blocking — the Load button stays enabled; this only flags the risk.
pub(super) fn vram_warning<'a>(needed: u64, available: u64) -> Element<'a, Message> {
    use crate::ui::icons;
    use cosmic::iced::Alignment;
    use cosmic::iced_widget::row;
    use cosmic::widget::text;

    row![
        icons::phosphor_warning(icons::WARNING, 16.0),
        text::body(format!(
            "This model may not fit on your GPU — needs ~{}, {} available.",
            fmt_gib(needed),
            fmt_gib(available),
        )),
    ]
    .spacing(cosmic::theme::spacing().space_xs)
    .align_y(Alignment::Center)
    .into()
}
