// SPDX-License-Identifier: GPL-3.0-only
//! Pluggable git-forge clients.

mod github;
pub mod http;
pub use github::Github;

use async_trait::async_trait;
use super_stt_registry_types::forge::Forge;
use thiserror::Error;

/// A parsed `<host>/<owner>/<repo>` reference. Unlike the old github-only
/// parsers, the host is retained (a future GitLab/Gitea adapter needs it, and
/// the custom-repo path checks the manifest's `source` against it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRef {
    pub host: String,
    pub owner: String,
    pub repo: String,
}

#[derive(Debug, Error)]
pub enum RepoRefError {
    #[error("repo `{0}` is not a `<host>/<owner>/<repo>` reference")]
    Invalid(String),
}

impl RepoRef {
    /// Parse `<host>/<owner>/<repo>`, tolerating an `https://`/`http://` scheme
    /// and a trailing `.git`, `/`, query, or fragment.
    ///
    /// # Errors
    /// [`RepoRefError::Invalid`] when the input is not exactly three non-empty
    /// path segments.
    pub fn parse(input: &str) -> Result<RepoRef, RepoRefError> {
        let s = input.trim();
        let after = s
            .strip_prefix("https://")
            .or_else(|| s.strip_prefix("http://"))
            .unwrap_or(s);
        let after = after.split('#').next().unwrap_or(after);
        let after = after.split('?').next().unwrap_or(after);
        let after = after.trim_end_matches('/');
        let after = after.strip_suffix(".git").unwrap_or(after);
        let segs: Vec<&str> = after.split('/').collect();
        if segs.len() != 3 || segs.iter().any(|p| p.is_empty()) {
            return Err(RepoRefError::Invalid(input.to_string()));
        }
        Ok(RepoRef {
            host: segs[0].to_string(),
            owner: segs[1].to_string(),
            repo: segs[2].to_string(),
        })
    }

    /// `<host>/<owner>/<repo>`, normalized (scheme/suffix stripped).
    #[must_use]
    pub fn canonical(&self) -> String {
        format!("{}/{}/{}", self.host, self.owner, self.repo)
    }
}

/// A forge-neutral release. Adapters map their host's JSON onto this.
#[derive(Debug, Clone)]
pub struct Release {
    pub tag: String,
    pub draft: bool,
    pub prerelease: bool,
    pub assets: Vec<ReleaseAsset>,
}

/// A forge-neutral release asset. `download_url` is a plain HTTPS URL.
#[derive(Debug, Clone)]
pub struct ReleaseAsset {
    pub name: String,
    pub download_url: String,
    pub size: u64,
}

#[derive(Debug, Error)]
pub enum ForgeError {
    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),
    #[error("response body exceeded the {limit}-byte cap")]
    TooLarge { limit: u64 },
}

impl ForgeError {
    /// The HTTP status of the underlying transport error, if any. Callers use
    /// this to distinguish "not found" (404) from other failures.
    #[must_use]
    pub fn http_status(&self) -> Option<reqwest::StatusCode> {
        match self {
            ForgeError::Http(e) => e.status(),
            ForgeError::TooLarge { .. } => None,
        }
    }
}

/// Release discovery + asset download for one git forge.
#[async_trait]
pub trait ForgeClient: Send + Sync {
    /// The repo's latest published release.
    ///
    /// # Errors
    /// Network failure or a non-2xx response from the forge.
    async fn latest_release(&self, repo: &RepoRef) -> Result<Release, ForgeError>;
    /// Up to one page of the repo's releases (newest first). Used by the
    /// indexer to select a tag-prefixed release in a monorepo.
    ///
    /// # Errors
    /// Network failure or a non-2xx response from the forge.
    async fn list_releases(&self, repo: &RepoRef) -> Result<Vec<Release>, ForgeError>;
    /// Download raw bytes from an asset URL, reusing the adapter's configured
    /// HTTP client (timeouts, redirects). The body is streamed with an
    /// accumulating cap: as soon as it would exceed `max_bytes` the download is
    /// aborted with [`ForgeError::TooLarge`], so a malicious forge/proxy can't
    /// OOM the caller by serving a huge body regardless of the declared size.
    ///
    /// # Errors
    /// Network failure, a non-2xx response, or a body over `max_bytes`.
    async fn download(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, ForgeError>;
}

/// Build the client for a forge. The match is exhaustive with no catch-all
/// arm, so adding a `Forge` variant fails to compile until its adapter exists.
#[must_use]
pub fn client(forge: Forge) -> Box<dyn ForgeClient> {
    match forge {
        Forge::Github => Box::new(Github::from_env()),
    }
}

/// Install the ring crypto provider for rustls.
/// Safe to call multiple times — returns `Ok` on first call, `Err` on
/// subsequent (which we ignore).
#[cfg(test)]
pub(crate) fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Whether an operator-provided API base URL may be used: `https://`, or a
/// loopback `http://` for local testing. Anything else is rejected so adapters
/// fall back to their secure default.
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
mod repo_ref_tests {
    use super::RepoRef;

    #[test]
    fn parses_and_keeps_host_owner_repo() {
        for raw in [
            "github.com/a/b",
            "https://github.com/a/b",
            "http://github.com/a/b",
            "https://github.com/a/b.git",
            "https://github.com/a/b/",
            "https://github.com/a/b?ref=main",
            "  https://github.com/a/b  ",
        ] {
            let r = RepoRef::parse(raw).unwrap();
            assert_eq!(
                (r.host.as_str(), r.owner.as_str(), r.repo.as_str()),
                ("github.com", "a", "b"),
                "{raw}"
            );
        }
        let r = RepoRef::parse("git.example.com/a/b").unwrap();
        assert_eq!(r.canonical(), "git.example.com/a/b");
    }

    #[test]
    fn rejects_wrong_segment_count_or_empty() {
        for bad in [
            "github.com/a",
            "github.com/a/b/c",
            "github.com//b",
            "github.com/a/",
            "a/b",
        ] {
            assert!(RepoRef::parse(bad).is_err(), "{bad} should be rejected");
        }
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::{Forge, client};

    #[test]
    fn dispatches_github() {
        crate::install_crypto_provider();
        let _c = client(Forge::Github);
    }
}
