// SPDX-License-Identifier: GPL-3.0-only
mod core;
mod daemon;
mod i18n;
mod state;
mod ui;

/// No CLI flags; the type exists only to satisfy `run_single_instance`'s
/// `CosmicFlags` bound. `action()` stays `None`, so a second launch
/// activates the running window rather than routing an action to it.
///
/// Still the model's `Flags` type where single-instance is off, so the
/// impl below stays unconditional even though nothing reads it there.
#[derive(Debug, Clone)]
pub struct Flags;

impl cosmic::app::CosmicFlags for Flags {
    type SubCommand = String;
    type Args = Vec<String>;
}

fn main() -> cosmic::iced::Result {
    super_stt_shared::logging::init();

    // Install the rustls crypto provider before any HTTP client is built —
    // the app's first direct download (the self-update installer binary,
    // via super-stt-forge) needs it.
    super_stt_forge::install_crypto_provider();

    // Get the system's preferred languages.
    let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();

    // Enable localizations to be applied.
    i18n::init(&requested_languages);

    // Settings for configuring the application window and iced runtime.
    let settings = cosmic::app::Settings::default().size_limits(
        cosmic::iced::Limits::NONE
            .min_width(360.0)
            .min_height(180.0),
    );

    // Run as a single-instance D-Bus-activated app. A second launch (e.g.
    // from the update notification's "Open Super STT" action) activates
    // and focuses the existing window instead of opening a duplicate.
    #[cfg(target_os = "linux")]
    return cosmic::app::run_single_instance::<core::AppModel>(settings, Flags);

    // libcosmic builds single-instance activation on zbus, so the feature is
    // off where there is no D-Bus (see this crate's Cargo.toml). Nothing
    // replaces it: a second launch opens a second window rather than
    // focusing the first.
    #[cfg(not(target_os = "linux"))]
    return cosmic::app::run::<core::AppModel>(settings, Flags);
}
