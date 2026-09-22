// SPDX-License-Identifier: GPL-3.0-only
//! One home for the Super STT base directories.
//!
//! Replaces the byte-identical daemon↔applet `get_config_path` cores and the
//! scattered `dirs`-miss fallbacks. Each helper returns the `super-stt`
//! subdirectory of its base, applying the same fallback the call sites used
//! (so behavior is unchanged) — callers append their own filename. The
//! validated runtime-socket path lives separately in
//! [`crate::validation`] (`get_http_socket_path` etc.).
//!
//! **Per-platform bases.** `dirs` gives the native location for each: the XDG
//! directories on Linux, `~/Library/…` on macOS. An explicitly set XDG
//! environment variable wins on both — see [`xdg_override`] for why that
//! matters beyond Linux.

use std::path::PathBuf;

/// An explicitly set XDG base-directory variable, when it names an absolute
/// path.
///
/// On Linux this changes nothing: `dirs` already reads these variables, so
/// this just gets there first with the same answer.
///
/// On macOS it is what makes the directories redirectable at all. `dirs`
/// returns `~/Library/…` there and ignores the environment entirely, which is
/// the right *default* — it is where a Mac user expects an application's data
/// to live — but it leaves no way to point a process somewhere else. The
/// daemon's own integration tests depend on exactly that: each spawns a real
/// daemon against a temp `XDG_DATA_HOME`/`XDG_CONFIG_HOME`/`XDG_CACHE_HOME`
/// so it discovers fixture backends and cannot disturb the developer's own
/// install. With the variables ignored, those tests do not merely fail — they
/// run the daemon against the developer's real `~/Library/Application
/// Support/super-stt`.
///
/// Absolute paths only. The XDG specification requires it, `dirs` enforces it
/// on Linux, and a relative value would otherwise resolve against whatever
/// directory the process happens to be in.
fn xdg_override(var: &str) -> Option<PathBuf> {
    let value = std::env::var_os(var)?;
    let path = PathBuf::from(value);
    (path.is_absolute()).then_some(path)
}

/// `$XDG_CONFIG_HOME/super-stt` (fallback: the platform's config directory —
/// `$HOME/.config` on Linux, `~/Library/Application Support` on macOS — else
/// `/tmp/.config/super-stt`). Daemon: append `daemon.toml`; applet: append
/// `applet-<variant>.toml`.
#[must_use]
pub fn config_dir() -> PathBuf {
    xdg_override("XDG_CONFIG_HOME")
        .or_else(dirs::config_dir)
        .unwrap_or_else(|| home_join(".config"))
        .join("super-stt")
}

/// `$XDG_DATA_HOME/super-stt` (fallback: the platform's data directory —
/// `$HOME/.local/share` on Linux, `~/Library/Application Support` on macOS —
/// else `/tmp/.local/share/super-stt`). Used for installed backends.
#[must_use]
pub fn data_dir() -> PathBuf {
    xdg_override("XDG_DATA_HOME")
        .or_else(dirs::data_dir)
        .unwrap_or_else(|| home_join(".local/share"))
        .join("super-stt")
}

/// `$XDG_CACHE_HOME/super-stt` (fallback: the platform's cache directory —
/// `$HOME/.cache` on Linux, `~/Library/Caches` on macOS — else
/// `$TMPDIR/super-stt`). Used for the registry index cache and staged
/// installs.
#[must_use]
pub fn cache_dir() -> PathBuf {
    xdg_override("XDG_CACHE_HOME")
        .or_else(dirs::cache_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("super-stt")
}

/// `$HOME/<suffix>`, falling back to `/tmp/<suffix>` when `HOME` is unset —
/// the shared fallback for the config/data dirs above.
fn home_join(suffix: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(suffix)
}

#[cfg(test)]
mod tests {
    use super::{cache_dir, config_dir, data_dir};

    /// Every base has to be redirectable by its XDG variable, on every
    /// platform.
    ///
    /// This is the property the daemon's integration tests rest on: they
    /// spawn a real daemon with these variables pointed at a tempdir. Where
    /// it does not hold, those tests run against the developer's own install
    /// instead of failing — which on macOS is exactly what happened before
    /// `xdg_override` existed, because `dirs` reads `~/Library/…` and ignores
    /// the environment.
    ///
    /// One test rather than three: they share the process environment, so
    /// separate tests setting and unsetting these would race each other.
    #[test]
    fn every_base_dir_honors_its_xdg_override() {
        let root = std::env::temp_dir().join("super-stt-paths-test");
        for (var, dir) in [
            ("XDG_CONFIG_HOME", config_dir as fn() -> std::path::PathBuf),
            ("XDG_DATA_HOME", data_dir),
            ("XDG_CACHE_HOME", cache_dir),
        ] {
            unsafe {
                std::env::set_var(var, &root);
            }
            assert_eq!(
                dir(),
                root.join("super-stt"),
                "{var} did not redirect its base directory"
            );

            // A relative value is refused rather than resolved against the
            // process's working directory.
            unsafe {
                std::env::set_var(var, "relative/path");
            }
            assert_ne!(
                dir(),
                std::path::PathBuf::from("relative/path").join("super-stt"),
                "{var} accepted a relative path"
            );

            unsafe {
                std::env::remove_var(var);
            }
        }
    }

    /// With nothing set, each base must still land somewhere absolute under
    /// the user's own tree — the platform default, whatever it is.
    #[test]
    fn defaults_are_absolute_and_named_for_the_app() {
        for var in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"] {
            unsafe {
                std::env::remove_var(var);
            }
        }
        for dir in [config_dir(), data_dir(), cache_dir()] {
            assert!(dir.is_absolute(), "{} is not absolute", dir.display());
            assert_eq!(dir.file_name().and_then(|n| n.to_str()), Some("super-stt"));
        }
    }
}
