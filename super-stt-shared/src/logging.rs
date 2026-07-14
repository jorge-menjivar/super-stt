// SPDX-License-Identifier: GPL-3.0-only
//! One `env_logger` initializer for every Super STT binary.
//!
//! Replaces five near-identical setups (and one that silently defaulted to
//! `error`, plus a CLI with none at all). `RUST_LOG` always wins; otherwise the
//! level falls back to the caller's default. Call this ONCE, as early in `main`
//! as possible — before any config load — so startup diagnostics (e.g. a
//! "config invalid, reset to defaults" warning) aren't emitted before the logger
//! exists and dropped.

/// Initialize logging with an `Info` default level. `RUST_LOG` still overrides.
pub fn init() {
    init_with(log::LevelFilter::Info);
}

/// Initialize logging with an explicit default level. `RUST_LOG` still
/// overrides (e.g. the daemon passes `Debug` under `--verbose`).
pub fn init_with(default_level: log::LevelFilter) {
    if std::env::var_os("RUST_LOG").is_some() {
        env_logger::init();
    } else {
        env_logger::Builder::from_default_env()
            .filter_level(default_level)
            .init();
    }
}
