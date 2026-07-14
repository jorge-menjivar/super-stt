// SPDX-License-Identifier: GPL-3.0-only
//! Secure path helpers: client-ID generation and socket-path construction.

/// Generate a cryptographically secure client ID
///
/// This function generates a unique client ID that prevents prediction and impersonation attacks.
///
/// Security features:
/// - UUID v4 for cryptographic randomness
/// - High-resolution timestamp for temporal uniqueness
/// - Process ID for system-level uniqueness
/// - Multi-factor composition for collision resistance
///
/// Format: `{component}-{pid}-{timestamp}-{uuid}`
#[must_use]
pub fn generate_secure_client_id(component: &str) -> String {
    let pid = std::process::id();
    let uuid = uuid::Uuid::new_v4();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::ZERO)
        .as_nanos();
    format!("{component}-{pid}-{timestamp}-{uuid}")
}

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

/// Get a secure socket path for the length-prefix protocol socket (`super-stt.sock`).
///
/// Validates the `XDG_RUNTIME_DIR` environment variable and constructs a secure
/// socket path that prevents path injection attacks.
///
/// Security features:
/// - Path length validation
/// - Path traversal prevention
/// - Directory whitelist enforcement
/// - Canonical path verification
/// - Secure fallback behavior
/// - Security event logging
#[must_use]
pub fn get_secure_socket_path() -> std::path::PathBuf {
    secure_runtime_path("super-stt.sock")
}

/// Get the path of the HTTP-protocol Unix socket (`super-stt-http.sock`).
///
/// This sits next to the legacy length-prefix socket from `get_secure_socket_path`,
/// in the same `$XDG_RUNTIME_DIR/stt/` directory. The two listeners run
/// side-by-side; clients pick whichever one matches their protocol.
#[must_use]
pub fn get_http_socket_path() -> std::path::PathBuf {
    secure_runtime_path("super-stt-http.sock")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_secure_client_id() {
        // Test that client IDs are unique
        let id1 = generate_secure_client_id("test-app");
        let id2 = generate_secure_client_id("test-app");
        assert_ne!(id1, id2, "Client IDs must be unique");

        // Test that client IDs contain the component name
        let app_id = generate_secure_client_id("super-stt-app");
        assert!(
            app_id.starts_with("super-stt-app-"),
            "Client ID should start with component name"
        );

        let applet_id = generate_secure_client_id("super-stt-applet");
        assert!(
            applet_id.starts_with("super-stt-applet-"),
            "Client ID should start with component name"
        );

        // Test that client IDs have expected format (component-pid-timestamp-uuid)
        let parts: Vec<&str> = app_id.split('-').collect();
        assert!(
            parts.len() >= 6,
            "Client ID should have at least 6 parts separated by hyphens"
        );

        // Test that the UUID part is valid (36 characters with hyphens)
        let uuid_part = parts[parts.len() - 5..].join("-");
        assert_eq!(uuid_part.len(), 36, "UUID part should be 36 characters");
    }

    #[test]
    fn test_get_secure_socket_path() {
        // Test with valid XDG_RUNTIME_DIR
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        }
        let path = get_secure_socket_path();
        assert!(path.to_string_lossy().contains("super-stt.sock"));

        // Test with potentially malicious XDG_RUNTIME_DIR (path traversal)
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", "../../../etc");
        }
        let fallback_path = get_secure_socket_path();
        assert_eq!(
            fallback_path,
            std::path::PathBuf::from("/tmp/stt/super-stt.sock")
        );

        // Test with invalid directory prefix
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", "/etc/passwd");
        }
        let fallback_path2 = get_secure_socket_path();
        assert_eq!(
            fallback_path2,
            std::path::PathBuf::from("/tmp/stt/super-stt.sock")
        );

        // Test with extremely long path
        let long_path = "a".repeat(300);
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", &long_path);
        }
        let fallback_path3 = get_secure_socket_path();
        assert_eq!(
            fallback_path3,
            std::path::PathBuf::from("/tmp/stt/super-stt.sock")
        );

        // Clean up environment
        unsafe {
            std::env::remove_var("XDG_RUNTIME_DIR");
        }
    }
}
