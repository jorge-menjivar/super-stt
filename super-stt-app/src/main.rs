// SPDX-License-Identifier: GPL-3.0-only
mod audio;
mod core;
mod daemon;
mod i18n;
mod state;
mod ui;

fn main() -> cosmic::iced::Result {
    // Initialize logging - respect RUST_LOG env var, fallback to verbose flag
    if std::env::var("RUST_LOG").is_ok() {
        env_logger::init();
    } else {
        let log_level = log::LevelFilter::Info;
        env_logger::Builder::from_default_env()
            .filter_level(log_level)
            .init();
    }

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
