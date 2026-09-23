// SPDX-License-Identifier: GPL-3.0-only
//! Secure path helpers: socket-path construction under the per-user runtime
//! directory — `$XDG_RUNTIME_DIR/stt/` on Linux, the Darwin per-user temp
//! directory's `stt/` on macOS.

/// The longest path a pathname Unix socket may occupy, terminator included:
/// the size of `sockaddr_un::sun_path`. Linux gives 108 bytes, macOS 104.
/// Callers that mint a socket *name* budget against this — see the subprocess
/// backend's instance key.
#[cfg(target_os = "linux")]
pub const SUN_PATH_MAX: usize = 108;
/// See [`SUN_PATH_MAX`].
#[cfg(target_os = "macos")]
pub const SUN_PATH_MAX: usize = 104;

/// Directory prefixes a runtime path is allowed to resolve under.
///
/// The point is not that these are the only writable directories, but that
/// each is per-user and not attacker-controlled, so a socket bound below one
/// cannot be pre-created or swapped by another local user.
///
/// On Linux that is the systemd-provided `/run/user/<uid>`, plus `/tmp` for
/// the fallback and for hosts without a runtime dir.
#[cfg(target_os = "linux")]
const ALLOWED_PREFIXES: &[&str] = &["/run/user/", "/tmp/"];

/// See [`ALLOWED_PREFIXES`]. macOS has no `/run/user`; the equivalent is the
/// Darwin per-user temp directory, `/var/folders/<xx>/<hash>/T/`, which the
/// OS creates mode 0700 and owned by the user. Both spellings are listed
/// because `/var` is a symlink to `/private/var`, so canonicalizing any path
/// under it rewrites the prefix — likewise `/tmp` → `/private/tmp`.
#[cfg(target_os = "macos")]
const ALLOWED_PREFIXES: &[&str] = &[
    "/var/folders/",
    "/private/var/folders/",
    "/tmp/",
    "/private/tmp/",
];

/// The per-user runtime directory this platform puts sockets in, before any
/// validation.
///
/// Linux: `$XDG_RUNTIME_DIR`, falling back to the `/run/user/<uid>` systemd
/// would have set it to.
#[cfg(target_os = "linux")]
fn runtime_dir_hint() -> String {
    std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/run/user/{}", unsafe { libc::getuid() }))
}

/// See [`runtime_dir_hint`].
///
/// macOS: `$XDG_RUNTIME_DIR` is honored when set — nothing on macOS sets it,
/// but the daemon's own integration tests do, and an override that works on
/// one platform should work on both. Otherwise the Darwin per-user temp
/// directory, read from `confstr(_CS_DARWIN_USER_TEMP_DIR)` rather than
/// `$TMPDIR`.
///
/// `confstr` rather than the environment variable because the two disagree
/// exactly when it matters: `$TMPDIR` is inherited, so a daemon started from
/// a shell with `TMPDIR` overridden — or a launchd agent, which is handed a
/// different environment entirely — would bind somewhere its clients do not
/// look. `confstr` asks the kernel and gives every process of this user the
/// same answer.
#[cfg(target_os = "macos")]
fn runtime_dir_hint() -> String {
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR")
        && !xdg.is_empty()
    {
        return xdg;
    }
    darwin_user_temp_dir().unwrap_or_else(|| "/tmp".to_string())
}

/// `confstr(_CS_DARWIN_USER_TEMP_DIR)` — the per-user temp directory, e.g.
/// `/var/folders/xt/<hash>/T/`, with its trailing slash trimmed.
///
/// `None` when `confstr` reports the name is unavailable (it returns 0 and
/// sets `errno`), which in practice means a stripped-down environment with no
/// per-user directory provisioned. The caller degrades to `/tmp`.
#[cfg(target_os = "macos")]
fn darwin_user_temp_dir() -> Option<String> {
    // First call with a null buffer asks for the required size, terminator
    // included. `confstr` returns 0 for an unsupported name.
    let needed = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, std::ptr::null_mut(), 0) };
    if needed == 0 {
        return None;
    }
    let mut buf = vec![0u8; needed];
    let written = unsafe {
        libc::confstr(
            libc::_CS_DARWIN_USER_TEMP_DIR,
            buf.as_mut_ptr().cast::<libc::c_char>(),
            buf.len(),
        )
    };
    // A second call that needs *more* room than the first reported means the
    // value changed underneath us; treat it as unavailable rather than
    // returning a truncated directory that would silently be the wrong one.
    if written == 0 || written > buf.len() {
        return None;
    }
    // `written` counts the NUL terminator.
    buf.truncate(written - 1);
    let dir = String::from_utf8(buf).ok()?;
    Some(dir.trim_end_matches('/').to_string())
}

/// Build a validated runtime path `<runtime dir>/stt/<relative>` with
/// path-traversal / prefix / length checks on the runtime dir and a
/// `/tmp/stt/<relative>` fallback. `relative` is a caller-controlled subpath
/// (a bare filename, or e.g. `backends/<name>.sock`) joined after `stt/`.
///
/// The runtime dir is `$XDG_RUNTIME_DIR` on Linux and the Darwin per-user
/// temp directory on macOS — see [`runtime_dir_hint`].
///
/// Shared entry point for every runtime socket so callers can't bypass the
/// SSRF/traversal guards with a hand-rolled runtime-dir join.
#[must_use]
pub fn secure_runtime_path(relative: &str) -> std::path::PathBuf {
    let fallback = || std::path::PathBuf::from(format!("/tmp/stt/{relative}"));
    let runtime_dir = runtime_dir_hint();

    if runtime_dir.is_empty() || runtime_dir.len() > 256 {
        log::warn!("Invalid runtime dir length, using fallback");
        return fallback();
    }
    if runtime_dir.contains("..") || runtime_dir.contains('\0') {
        log::warn!("Potential path traversal in runtime dir, using fallback");
        return fallback();
    }
    if !is_allowed(&runtime_dir) {
        log::warn!("Runtime dir outside allowed directories: {runtime_dir}, using fallback");
        return fallback();
    }

    let path = std::path::PathBuf::from(runtime_dir)
        .join("stt")
        .join(relative);
    if let Ok(canonical) = path.canonicalize() {
        if !is_allowed(&canonical.to_string_lossy()) {
            log::warn!("Canonical runtime path {relative} outside allowed directories, fallback");
            return fallback();
        }
        canonical
    } else {
        path
    }
}

/// Whether `path` sits under one of [`ALLOWED_PREFIXES`].
fn is_allowed(path: &str) -> bool {
    ALLOWED_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix))
}

/// Get the path of the HTTP-protocol Unix socket (`super-stt-http.sock`) — the
/// daemon's sole client-facing listener, which all clients connect to.
///
/// A non-empty `SUPER_STT_HTTP_SOCKET` overrides the path verbatim (tests use
/// this to bind a unique socket per run without touching the runtime dir).
/// Both the daemon and every client resolve their path through here, so the
/// override applies uniformly — set it and both ends agree. When unset, the
/// path is `<runtime dir>/stt/super-stt-http.sock` via [`secure_runtime_path`],
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

    /// A runtime dir this platform accepts, used as the "honored" case below.
    #[cfg(target_os = "linux")]
    const GOOD_RUNTIME_DIR: &str = "/run/user/1000";
    #[cfg(target_os = "macos")]
    const GOOD_RUNTIME_DIR: &str = "/tmp/stt-test-runtime";

    #[test]
    fn secure_runtime_path_guards_runtime_dir() {
        // A runtime dir on the allowlist is honored.
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", GOOD_RUNTIME_DIR);
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

        // With no override at all, the platform default must still land
        // somewhere the guards accept. On macOS this is the regression that
        // matters most: the Linux default is `/run/user/<uid>`, which does
        // not exist there, so a daemon that kept it would fail to bind on
        // every start.
        //
        // Asserted here, inside the test that already owns
        // `XDG_RUNTIME_DIR`, rather than as tests of its own. Tests share one
        // process, so a separate test unsetting the variable would race the
        // cases above that set it.
        unsafe {
            std::env::remove_var("XDG_RUNTIME_DIR");
        }
        let path = secure_runtime_path("super-stt-http.sock");
        let rendered = path.to_string_lossy().into_owned();
        assert!(
            is_allowed(&rendered),
            "default runtime path {rendered} is not under an allowed prefix"
        );
        assert!(rendered.ends_with("stt/super-stt-http.sock"));

        // And it has to fit in `sun_path`. The default is the longest path
        // that is not caller-influenced, so if it does not fit, nothing will.
        let len = path.as_os_str().len();
        assert!(
            len < SUN_PATH_MAX,
            "default socket path is {len} bytes, sun_path holds {SUN_PATH_MAX} including the terminator"
        );
    }

    /// The Darwin per-user temp dir is what every macOS socket hangs off, so
    /// it has to be readable and has to be a directory that actually exists.
    #[cfg(target_os = "macos")]
    #[test]
    fn darwin_user_temp_dir_resolves() {
        let dir = darwin_user_temp_dir().expect("confstr(_CS_DARWIN_USER_TEMP_DIR)");
        assert!(
            std::path::Path::new(&dir).is_dir(),
            "{dir} is not a directory"
        );
        assert!(!dir.ends_with('/'), "{dir} kept its trailing slash");
        assert!(is_allowed(&dir), "{dir} is not under an allowed prefix");
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
