// SPDX-License-Identifier: GPL-3.0-only
//! Super STT's base directories: [`super_engine_protocol::paths`] for
//! [`SUPER_STT`]. Callers append their own filename. The runtime socket path
//! lives in [`crate::validation`] (`get_http_socket_path` etc.).

use std::path::PathBuf;

use super_engine_protocol::SUPER_STT;
use super_engine_protocol::paths;

/// `$XDG_CONFIG_HOME/super-stt`, with the fallbacks
/// [`paths::config_dir`] names. Daemon: append `daemon.toml`; applet: append
/// `applet-<variant>.toml`.
#[must_use]
pub fn config_dir() -> PathBuf {
    paths::config_dir(&SUPER_STT)
}

/// `$XDG_DATA_HOME/super-stt`, with the fallbacks [`paths::data_dir`] names.
/// Used for installed backends.
#[must_use]
pub fn data_dir() -> PathBuf {
    paths::data_dir(&SUPER_STT)
}

/// `$XDG_CACHE_HOME/super-stt`, with the fallbacks [`paths::cache_dir`]
/// names. Used for the registry index cache and staged installs.
#[must_use]
pub fn cache_dir() -> PathBuf {
    paths::cache_dir(&SUPER_STT)
}
