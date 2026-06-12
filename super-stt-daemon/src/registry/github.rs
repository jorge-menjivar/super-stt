// SPDX-License-Identifier: GPL-3.0-only
//! Minimal GitHub REST client for the Custom-repo install path.
//!
//! Mirrors `super-stt-indexer/src/github.rs` shape-wise, but uses
//! `thiserror` so failures map cleanly to install-pipeline errors. Auth is
//! optional and only used to lift rate limits when `GITHUB_TOKEN` is set on
//! the daemon's environment.

use std::time::Duration;

use base64::Engine;
use serde::Deserialize;
use thiserror::Error;

const DEFAULT_BASE: &str = "https://api.github.com";

#[derive(Debug, Error)]
pub enum GitHubError {
    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),
    #[error("response is not base64-encoded: encoding=`{0}`")]
    UnexpectedEncoding(String),
    #[error("base64 decode: {0}")]
    Base64(#[from] base64::DecodeError),
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

#[derive(Debug, Deserialize)]
struct ContentResponse {
    content: String,
    encoding: String,
}

#[derive(Clone)]
pub struct GitHub {
    base: String,
    http: reqwest::Client,
    token: Option<String>,
}

impl GitHub {
    pub fn new(base: impl Into<String>, token: Option<String>) -> Self {
        Self {
            base: base.into(),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::limited(5))
                .build()
                .unwrap_or_default(),
            token,
        }
    }

    #[must_use]
    pub fn from_env() -> Self {
        let base = match std::env::var("GITHUB_API_BASE") {
            Ok(v) if crate::registry::accept_base_url(&v) => v,
            Ok(v) => {
                log::warn!("ignoring insecure GITHUB_API_BASE={v:?}; using {DEFAULT_BASE}");
                DEFAULT_BASE.into()
            }
            Err(_) => DEFAULT_BASE.into(),
        };
        Self::new(base, std::env::var("GITHUB_TOKEN").ok())
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut b = self
            .http
            .request(method, format!("{}{path}", self.base))
            .header("User-Agent", "super-stt-daemon-install/0.1")
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(t) = &self.token {
            b = b.bearer_auth(t);
        }
        b
    }

    /// `GET /repos/{owner_repo}/releases/latest`.
    ///
    /// # Errors
    /// Network failure, non-2xx response (incl. 404 for "no releases").
    pub async fn latest_release(&self, owner_repo: &str) -> Result<Release, GitHubError> {
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

    /// `GET /repos/{owner_repo}/contents/{path}?ref={ref}`, base64-decoded.
    ///
    /// # Errors
    /// Network failure, non-2xx response, or unexpected encoding.
    pub async fn fetch_file(
        &self,
        owner_repo: &str,
        path: &str,
        git_ref: &str,
    ) -> Result<Vec<u8>, GitHubError> {
        let r = self
            .req(
                reqwest::Method::GET,
                &format!("/repos/{owner_repo}/contents/{path}?ref={git_ref}"),
            )
            .send()
            .await?
            .error_for_status()?;
        let body: ContentResponse = r.json().await?;
        if body.encoding != "base64" {
            return Err(GitHubError::UnexpectedEncoding(body.encoding));
        }
        Ok(base64::engine::general_purpose::STANDARD.decode(body.content.replace('\n', ""))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn latest_release_returns_tag() {
        crate::install_crypto_provider();
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

    #[tokio::test]
    async fn fetch_file_decodes_base64() {
        crate::install_crypto_provider();
        let mut s = mockito::Server::new_async().await;
        s.mock(
            "GET",
            mockito::Matcher::Regex(r"^/repos/x/y/contents/backend\.toml.*".into()),
        )
        .with_status(200)
        .with_body(r#"{"content":"aGVsbG8=","encoding":"base64"}"#)
        .create_async()
        .await;
        let gh = GitHub::new(s.url(), None);
        let bytes = gh
            .fetch_file("x/y", "backend.toml", "v1.2.3")
            .await
            .unwrap();
        assert_eq!(&bytes, b"hello");
    }

    #[tokio::test]
    async fn fetch_file_rejects_non_base64_encoding() {
        crate::install_crypto_provider();
        let mut s = mockito::Server::new_async().await;
        s.mock("GET", mockito::Matcher::Any)
            .with_status(200)
            .with_body(r#"{"content":"hi","encoding":"utf-8"}"#)
            .create_async()
            .await;
        let gh = GitHub::new(s.url(), None);
        let err = gh
            .fetch_file("x/y", "backend.toml", "v1")
            .await
            .unwrap_err();
        assert!(matches!(err, GitHubError::UnexpectedEncoding(_)));
    }
}
