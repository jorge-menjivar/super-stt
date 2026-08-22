// SPDX-License-Identifier: GPL-3.0-only
mod core;
mod daemon;
mod i18n;
mod state;
mod ui;

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

    // Starts the application's event loop with `()` as the application's flags.
    cosmic::app::run::<core::AppModel>(settings, ())
}
