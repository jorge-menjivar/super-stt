// SPDX-License-Identifier: GPL-3.0-only
//! One home for the Super STT XDG base directories.
//!
//! Replaces the byte-identical daemon↔applet `get_config_path` cores and the
//! scattered `dirs`-miss fallbacks. Each helper returns the `super-stt`
//! subdirectory of its XDG base, applying the same fallback the call sites used
//! (so behavior is unchanged) — callers append their own filename. The
//! validated runtime-socket path lives separately in
//! [`crate::validation`] (`get_http_socket_path` etc.).

use std::path::PathBuf;

/// `$XDG_CONFIG_HOME/super-stt` (fallback `$HOME/.config/super-stt`, else
/// `/tmp/.config/super-stt`). Daemon: append `daemon.toml`; applet: append
/// `applet-<variant>.toml`.
#[must_use]
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| home_join(".config"))
        .join("super-stt")
}

/// `$XDG_DATA_HOME/super-stt` (fallback `$HOME/.local/share/super-stt`, else
/// `/tmp/.local/share/super-stt`). Used for installed backends.
#[must_use]
pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| home_join(".local/share"))
        .join("super-stt")
}

/// `$XDG_CACHE_HOME/super-stt` (fallback `$TMPDIR/super-stt`). Used for the
/// registry index cache and staged installs.
#[must_use]
pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("super-stt")
}

/// `$HOME/<suffix>`, falling back to `/tmp/<suffix>` when `HOME` is unset —
/// the shared fallback for the config/data dirs above.
fn home_join(suffix: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(suffix)
}
