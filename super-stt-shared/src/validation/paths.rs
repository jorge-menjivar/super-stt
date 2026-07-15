// SPDX-License-Identifier: GPL-3.0-only
//! Secure path helpers: socket-path construction under `$XDG_RUNTIME_DIR/stt/`.

/// Build a validated runtime path `$XDG_RUNTIME_DIR/stt/<relative>` with
/// path-traversal / prefix / length checks on the runtime dir and a
/// `/tmp/stt/<relative>` fallback. `relative` is a caller-controlled subpath
/// (a bare filename, or e.g. `backends/<name>.sock`) joined after `stt/`.
///
/// Shared entry point for every runtime socket so callers can't bypass the
/// SSRF/traversal guards with a hand-rolled `$XDG_RUNTIME_DIR` join.
#[must_use]
pub fn secure_runtime_path(relative: &str) -> std::path::PathBuf {
    let fallback = || std::path::PathBuf::from(format!("/tmp/stt/{relative}"));
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/run/user/{}", unsafe { libc::getuid() }));

    if runtime_dir.is_empty() || runtime_dir.len() > 256 {
        log::warn!("Invalid XDG_RUNTIME_DIR length, using fallback");
        return fallback();
    }
    if runtime_dir.contains("..") || runtime_dir.contains('\0') {
        log::warn!("Potential path traversal in XDG_RUNTIME_DIR, using fallback");
        return fallback();
    }
    if !runtime_dir.starts_with("/run/user/") && !runtime_dir.starts_with("/tmp/") {
        log::warn!("XDG_RUNTIME_DIR outside allowed directories: {runtime_dir}, using fallback");
        return fallback();
    }

    let path = std::path::PathBuf::from(runtime_dir)
        .join("stt")
        .join(relative);
    if let Ok(canonical) = path.canonicalize() {
        if !canonical.starts_with("/run/user/") && !canonical.starts_with("/tmp/") {
            log::warn!("Canonical runtime path {relative} outside allowed directories, fallback");
            return fallback();
        }
        canonical
    } else {
        path
    }
}

/// Get the path of the HTTP-protocol Unix socket (`super-stt-http.sock`) — the
/// daemon's sole client-facing listener, which all clients connect to.
///
/// A non-empty `SUPER_STT_HTTP_SOCKET` overrides the path verbatim (tests use
/// this to bind a unique socket per run without touching `$XDG_RUNTIME_DIR`).
/// Both the daemon and every client resolve their path through here, so the
/// override applies uniformly — set it and both ends agree. When unset, the
/// path is `$XDG_RUNTIME_DIR/stt/super-stt-http.sock` via [`secure_runtime_path`],
/// which applies the traversal / prefix / length guards.
#[must_use]
pub fn get_http_socket_path() -> std::path::PathBuf {
    if let Some(override_path) = std::env::var_os("SUPER_STT_HTTP_SOCKET")
        && !override_path.is_empty()
    {
        return std::path::PathBuf::from(override_path);
    }
    secure_runtime_path("super-stt-http.sock")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secure_runtime_path_guards_xdg_runtime_dir() {
        // Valid XDG_RUNTIME_DIR is honored.
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        }
        let path = secure_runtime_path("super-stt-http.sock");
        assert!(path.to_string_lossy().contains("super-stt-http.sock"));

        // Path traversal falls back to /tmp/stt/.
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", "../../../etc");
        }
        assert_eq!(
            secure_runtime_path("super-stt-http.sock"),
            std::path::PathBuf::from("/tmp/stt/super-stt-http.sock")
        );

        // Directory outside the whitelist falls back.
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", "/etc/passwd");
        }
        assert_eq!(
            secure_runtime_path("super-stt-http.sock"),
            std::path::PathBuf::from("/tmp/stt/super-stt-http.sock")
        );

        // Over-long dir falls back.
        let long_path = "a".repeat(300);
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", &long_path);
        }
        assert_eq!(
            secure_runtime_path("super-stt-http.sock"),
            std::path::PathBuf::from("/tmp/stt/super-stt-http.sock")
        );

        unsafe {
            std::env::remove_var("XDG_RUNTIME_DIR");
        }
    }

    #[test]
    fn http_socket_path_honors_env_override() {
        // A non-empty override is returned verbatim so the daemon and its
        // clients — all resolving through this helper — agree on the path.
        unsafe {
            std::env::set_var("SUPER_STT_HTTP_SOCKET", "/tmp/stt/custom-run.sock");
        }
        assert_eq!(
            get_http_socket_path(),
            std::path::PathBuf::from("/tmp/stt/custom-run.sock")
        );

        // Empty override is ignored — falls back to the runtime-dir path.
        unsafe {
            std::env::set_var("SUPER_STT_HTTP_SOCKET", "");
        }
        assert!(
            get_http_socket_path()
                .to_string_lossy()
                .ends_with("super-stt-http.sock")
        );

        unsafe {
            std::env::remove_var("SUPER_STT_HTTP_SOCKET");
        }
    }
}
