// SPDX-License-Identifier: GPL-3.0-only
//! Minimal GitHub REST client — only the endpoints the indexer needs.

use serde::Deserialize;

#[derive(Clone)]
pub struct GitHub {
    base: String,
    http: reqwest::Client,
    token: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

impl GitHub {
    pub fn new(base: impl Into<String>, token: Option<String>) -> Self {
        Self {
            base: base.into(),
            http: reqwest::Client::new(),
            token,
        }
    }

    pub fn from_env() -> Self {
        let base =
            std::env::var("GITHUB_API_BASE").unwrap_or_else(|_| "https://api.github.com".into());
        Self::new(base, std::env::var("GITHUB_TOKEN").ok())
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut b = self
            .http
            .request(method, format!("{}{path}", self.base))
            .header("User-Agent", "super-stt-indexer/0.1")
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(t) = &self.token {
            b = b.bearer_auth(t);
        }
        b
    }

    pub async fn latest_release(&self, owner_repo: &str) -> anyhow::Result<Release> {
        let r = self
            .req(
                reqwest::Method::GET,
                &format!("/repos/{owner_repo}/releases/latest"),
            )
            .send()
            .await?
            .error_for_status()?;
        Ok(r.json().await?)
    }

    pub async fn list_releases(&self, owner_repo: &str) -> anyhow::Result<Vec<Release>> {
        // GitHub paginates; 100 is the max page size. For the indexer use case
        // a single page is enough — backends with >100 releases are out of scope.
        let r = self
            .req(
                reqwest::Method::GET,
                &format!("/repos/{owner_repo}/releases?per_page=100"),
            )
            .send()
            .await?
            .error_for_status()?;
        Ok(r.json().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn latest_release_returns_tag() {
        let mut s = mockito::Server::new_async().await;
        s.mock("GET", "/repos/x/y/releases/latest")
            .with_status(200)
            .with_body(r#"{"tag_name":"v1.2.3","assets":[]}"#)
            .create_async()
            .await;
        let gh = GitHub::new(s.url(), None);
        let r = gh.latest_release("x/y").await.unwrap();
        assert_eq!(r.tag_name, "v1.2.3");
    }
}
