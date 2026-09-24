// SPDX-License-Identifier: GPL-3.0-only
//! Logging for every Super STT binary: super-engine's initializer, after the
//! macOS app bundle's log file is put in place.
//!
//! Call [`init`] or [`init_with`] ONCE, as early in `main` as possible. See
//! `super_engine_protocol::logging` for the rest.

/// Initialize logging with an `Info` default level. `RUST_LOG` still overrides.
pub fn init() {
    init_with(log::LevelFilter::Info);
}

/// Initialize logging with an explicit default level. `RUST_LOG` still
/// overrides (e.g. the daemon passes `Debug` under `--verbose`).
///
/// On macOS, output first goes to the file `SUPER_STT_LOG_FILE` names, which
/// the app bundle's `LaunchAgent` plists set. See
/// `super_engine_protocol::logging::redirect_stdio`.
pub fn init_with(default_level: log::LevelFilter) {
    #[cfg(target_os = "macos")]
    super_engine_protocol::logging::redirect_stdio(&crate::product::SUPER_STT);

    super_engine_protocol::logging::init_with(default_level);
}
