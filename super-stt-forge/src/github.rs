// SPDX-License-Identifier: GPL-3.0-only
//! GitHub adapter: maps the GitHub REST API onto the forge-neutral
//! `ForgeClient` trait. Merges what the indexer and the daemon's custom-repo
//! path previously each kept in their own `github.rs`.

use async_trait::async_trait;
use serde::Deserialize;

use crate::{ForgeClient, ForgeError, Release, ReleaseAsset, RepoRef};

const DEFAULT_BASE: &str = "https://api.github.com";

/// GitHub REST client. `base` is `https://api.github.com` by default, or a
/// GitHub Enterprise base via `GITHUB_API_BASE`. `token` lifts rate limits.
#[derive(Clone)]
pub struct Github {
    base: String,
    http: reqwest::Client,
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

impl From<GhRelease> for Release {
    fn from(r: GhRelease) -> Self {
        Release {
            tag: r.tag_name,
            draft: r.draft,
            prerelease: r.prerelease,
            assets: r.assets.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<GhAsset> for ReleaseAsset {
    fn from(a: GhAsset) -> Self {
        ReleaseAsset {
            name: a.name,
            download_url: a.browser_download_url,
            size: a.size,
        }
    }
}

impl Github {
    /// Construct a client for `base` with an optional bearer `token`.
    ///
    /// # Panics
    /// If the underlying reqwest client cannot be built (e.g. a TLS backend
    /// initialization failure) — not expected in practice.
    #[must_use]
    pub fn new(base: impl Into<String>, token: Option<String>) -> Self {
        Self {
            base: base.into(),
            http: crate::http::short_client(),
            token,
        }
    }

    /// Build from the environment: `GITHUB_API_BASE` (validated `https`/loopback,
    /// else the secure default) and an optional `GITHUB_TOKEN`.
    #[must_use]
    pub fn from_env() -> Self {
        let base = match std::env::var("GITHUB_API_BASE") {
            Ok(v) if crate::accept_base_url(&v) => v,
            Ok(v) => {
                log::warn!("ignoring insecure GITHUB_API_BASE={v:?}; using {DEFAULT_BASE}");
                DEFAULT_BASE.into()
            }
            Err(_) => DEFAULT_BASE.into(),
        };
        Self::new(base, std::env::var("GITHUB_TOKEN").ok())
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        // User-Agent is set at the client level (the workspace UA); GitHub only
        // requires it to be present and non-empty.
        let mut b = self
            .http
            .request(method, format!("{}{path}", self.base))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(t) = &self.token {
            b = b.bearer_auth(t);
        }
        b
    }
}

#[async_trait]
impl ForgeClient for Github {
    async fn latest_release(&self, repo: &RepoRef) -> Result<Release, ForgeError> {
        let r = self
            .req(
                reqwest::Method::GET,
                &format!("/repos/{}/{}/releases/latest", repo.owner, repo.repo),
            )
            .send()
            .await?
            .error_for_status()?;
        Ok(r.json::<GhRelease>().await?.into())
    }

    async fn list_releases(&self, repo: &RepoRef) -> Result<Vec<Release>, ForgeError> {
        // GitHub paginates; 100 is the max page size. A single page is enough
        // for the indexer — backends with >100 releases are out of scope.
        let r = self
            .req(
                reqwest::Method::GET,
                &format!("/repos/{}/{}/releases?per_page=100", repo.owner, repo.repo),
            )
            .send()
            .await?
            .error_for_status()?;
        Ok(r.json::<Vec<GhRelease>>()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn download(&self, url: &str) -> Result<Vec<u8>, ForgeError> {
        let r = self.http.get(url).send().await?.error_for_status()?;
        Ok(r.bytes().await?.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::Github;
    use crate::{ForgeClient, RepoRef};

    #[tokio::test]
    async fn latest_release_maps_github_json_to_neutral_release() {
        crate::install_crypto_provider();
        let mut s = mockito::Server::new_async().await;
        s.mock("GET", "/repos/x/y/releases/latest")
            .with_status(200)
            .with_body(
                r#"{"tag_name":"v1.2.3","assets":[{"name":"a.tar.gz","browser_download_url":"https://dl/a","size":7}]}"#,
            )
            .create_async()
            .await;
        let gh = Github::new(s.url(), None);
        let repo = RepoRef::parse("github.com/x/y").unwrap();
        let r = gh.latest_release(&repo).await.unwrap();
        assert_eq!(r.tag, "v1.2.3");
        assert!(!r.draft, "draft defaults to false when omitted");
        assert!(!r.prerelease, "prerelease defaults to false when omitted");
        assert_eq!(r.assets.len(), 1);
        assert_eq!(r.assets[0].name, "a.tar.gz");
        assert_eq!(r.assets[0].download_url, "https://dl/a");
        assert_eq!(r.assets[0].size, 7);
    }

    #[tokio::test]
    async fn list_releases_returns_all_tags() {
        crate::install_crypto_provider();
        let mut s = mockito::Server::new_async().await;
        s.mock("GET", "/repos/x/y/releases?per_page=100")
            .with_status(200)
            .with_body(r#"[{"tag_name":"v1.1.0"},{"tag_name":"v1.0.0"}]"#)
            .create_async()
            .await;
        let gh = Github::new(s.url(), None);
        let repo = RepoRef::parse("github.com/x/y").unwrap();
        let rels = gh.list_releases(&repo).await.unwrap();
        assert_eq!(rels.len(), 2);
        // Adapter preserves GitHub's newest-first order.
        assert_eq!(rels[0].tag, "v1.1.0");
        assert_eq!(rels[1].tag, "v1.0.0");
    }

    #[tokio::test]
    async fn download_returns_body_bytes() {
        crate::install_crypto_provider();
        let mut s = mockito::Server::new_async().await;
        s.mock("GET", "/asset")
            .with_status(200)
            .with_body("hello")
            .create_async()
            .await;
        let gh = Github::new(s.url(), None);
        let bytes = gh.download(&format!("{}/asset", s.url())).await.unwrap();
        assert_eq!(&bytes, b"hello");
    }

    #[tokio::test]
    async fn not_found_surfaces_as_404_status() {
        crate::install_crypto_provider();
        let mut s = mockito::Server::new_async().await;
        s.mock("GET", "/repos/x/y/releases/latest")
            .with_status(404)
            .create_async()
            .await;
        let gh = Github::new(s.url(), None);
        let repo = RepoRef::parse("github.com/x/y").unwrap();
        let err = gh.latest_release(&repo).await.unwrap_err();
        assert_eq!(err.http_status(), Some(reqwest::StatusCode::NOT_FOUND));
    }
}
