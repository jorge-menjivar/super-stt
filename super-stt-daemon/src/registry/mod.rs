// SPDX-License-Identifier: GPL-3.0-only
//! Daemon-side registry client, compatibility evaluation, and install pipeline.

pub mod client;
pub mod compat;
pub mod custom_repo;
pub mod github;
pub mod host_detect;
pub mod index_schema;
pub mod install;
pub mod local_dir;

/// Whether an operator-provided base URL (`GITHUB_API_BASE`,
/// `SUPER_STT_REGISTRY_URL`) may be used. Requires `https://`, except a
/// loopback `http://` URL which is permitted for local testing. Anything else
/// is rejected so callers fall back to their secure default.
#[must_use]
pub(crate) fn accept_base_url(url: &str) -> bool {
    if let Some(rest) = url.strip_prefix("https://") {
        !rest.is_empty()
    } else if let Some(rest) = url.strip_prefix("http://") {
        rest.starts_with("localhost") || rest.starts_with("127.0.0.1") || rest.starts_with("[::1]")
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::accept_base_url;

    #[test]
    fn accepts_https_and_loopback_http_only() {
        assert!(accept_base_url("https://api.github.com"));
        assert!(accept_base_url("http://localhost:8787"));
        assert!(accept_base_url("http://127.0.0.1:9000"));
        assert!(!accept_base_url("http://evil.example.com"));
        assert!(!accept_base_url("ftp://x"));
        assert!(!accept_base_url("https://"));
    }
}
