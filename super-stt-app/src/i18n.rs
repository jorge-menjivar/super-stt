// SPDX-License-Identifier: GPL-3.0-only

//! Provides localization support for this crate.

use std::sync::LazyLock;

use i18n_embed::{
    DefaultLocalizer, LanguageLoader, Localizer,
    fluent::{FluentLanguageLoader, fluent_language_loader},
    unic_langid::LanguageIdentifier,
};
use rust_embed::RustEmbed;

/// Applies the requested language(s) to requested translations from the `fl!()` macro.
pub fn init(requested_languages: &[LanguageIdentifier]) {
    if let Err(why) = localizer().select(requested_languages) {
        eprintln!("error while loading fluent localizations: {why}");
    }
}

// Get the `Localizer` to be used for localizing this library.
#[must_use]
pub fn localizer() -> Box<dyn Localizer> {
    Box::from(DefaultLocalizer::new(&*LANGUAGE_LOADER, &Localizations))
}

#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

pub static LANGUAGE_LOADER: LazyLock<FluentLanguageLoader> = LazyLock::new(|| {
    let loader: FluentLanguageLoader = fluent_language_loader!();

    loader
        .load_fallback_language(&Localizations)
        .expect("Error while loading fallback language");

    loader
});

/// Request a localized string by ID from the i18n/ directory.
#[macro_export]
macro_rules! fl {
    ($message_id:literal) => {{
        i18n_embed_fl::fl!($crate::i18n::LANGUAGE_LOADER, $message_id)
    }};

    ($message_id:literal, $($args:expr),*) => {{
        i18n_embed_fl::fl!($crate::i18n::LANGUAGE_LOADER, $message_id, $($args), *)
    }};
}

/// Localized display names stay in the UI; the shared enums keep their wire values.
pub fn recording_stop_mode_name(
    mode: super_stt_shared::models::recording_stop_mode::RecordingStopMode,
) -> String {
    use super_stt_shared::models::recording_stop_mode::RecordingStopMode;
    match mode {
        RecordingStopMode::SilenceOnly => crate::fl!("stop-silence"),
        RecordingStopMode::SilenceAndManual => crate::fl!("stop-silence-manual"),
        RecordingStopMode::ManualOnly => crate::fl!("stop-manual"),
    }
}

pub fn notification_method_name(
    method: super_stt_shared::models::notification_method::NotificationMethod,
) -> String {
    use super_stt_shared::models::notification_method::NotificationMethod;
    match method {
        NotificationMethod::Auto => crate::fl!("auto-recommended"),
        NotificationMethod::Desktop => crate::fl!("desktop-notification"),
        NotificationMethod::Typed => crate::fl!("type-into-window"),
        NotificationMethod::Off => crate::fl!("off"),
    }
}

pub fn write_method_name(method: super_stt_shared::models::write_method::WriteMethod) -> String {
    use super_stt_shared::models::write_method::WriteMethod;
    match method {
        WriteMethod::Auto => crate::fl!("auto-recommended"),
        WriteMethod::XdgDesktopPortal => crate::fl!("xdg-desktop-portal"),
        WriteMethod::Ydotool => "ydotool".to_string(),
        WriteMethod::BuiltIn => crate::fl!("built-in"),
    }
}

pub fn audio_theme_name(theme: super_stt_shared::models::theme::AudioTheme) -> String {
    use super_stt_shared::models::theme::AudioTheme;
    match theme {
        AudioTheme::Classic => crate::fl!("theme-classic"),
        AudioTheme::Gentle => crate::fl!("theme-gentle"),
        AudioTheme::Minimal => crate::fl!("theme-minimal"),
        AudioTheme::SciFi => crate::fl!("theme-scifi"),
        AudioTheme::Musical => crate::fl!("theme-musical"),
        AudioTheme::Nature => crate::fl!("theme-nature"),
        AudioTheme::Retro => crate::fl!("theme-retro"),
        AudioTheme::Silent => crate::fl!("theme-silent"),
    }
}

#[cfg(test)]
mod tests;
