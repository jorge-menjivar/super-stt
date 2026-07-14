// SPDX-License-Identifier: GPL-3.0-only
//! Fetch and cache the registry's `index.json`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::registry::index_schema::{Index, retain_safe_backends, warn_if_client_too_old};

pub const DEFAULT_URL: &str = "https://jorge-menjivar.github.io/super-stt/index.json";
pub const DEFAULT_TTL: Duration = Duration::from_hours(6);

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("registry unavailable and no cache")]
    Unavailable,
}

#[derive(Clone)]
pub struct Client {
    url: String,
    http: reqwest::Client,
    cache_path: PathBuf,
    ttl: Duration,
    state: Arc<RwLock<Option<Cached>>>,
}

#[derive(Clone)]
struct Cached {
    index: Index,
    etag: Option<String>,
    fetched_at: SystemTime,
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheFile {
    etag: Option<String>,
    fetched_at_secs: u64,
    index: serde_json::Value,
}

impl Client {
    /// # Panics
    /// Panics if the `reqwest` client cannot be built (should never happen with default settings).
    pub fn new(url: impl Into<String>, cache_path: PathBuf, ttl: Duration) -> Self {
        Self {
            url: url.into(),
            http: super_stt_forge::http::short_client(),
            cache_path,
            ttl,
            state: Arc::default(),
        }
    }

    #[must_use]
    pub fn from_env() -> Self {
        let url = match std::env::var("SUPER_STT_REGISTRY_URL") {
            Ok(v) if crate::registry::accept_base_url(&v) => v,
            Ok(v) => {
                log::warn!("ignoring insecure SUPER_STT_REGISTRY_URL={v:?}; using {DEFAULT_URL}");
                DEFAULT_URL.into()
            }
            Err(_) => DEFAULT_URL.into(),
        };
        let cache_dir = super_stt_shared::paths::cache_dir();
        let _ = std::fs::create_dir_all(&cache_dir);
        Self::new(url, cache_dir.join("registry-index.json"), DEFAULT_TTL)
    }

    /// Get the index. Uses memory → file cache → network in that order; falls
    /// back to whichever is freshest if the network is down.
    ///
    /// # Errors
    /// Returns `ClientError::Unavailable` when the network is unreachable and there is no
    /// cached index on disk.
    pub async fn get(&self) -> Result<Index, ClientError> {
        {
            let guard = self.state.read();
            if let Some(c) = guard.as_ref()
                && c.fetched_at.elapsed().unwrap_or_default() < self.ttl
            {
                return Ok(c.index.clone());
            }
        }
        if let Ok(idx) = self.refresh().await {
            Ok(idx)
        } else {
            let guard = self.state.read();
            if let Some(c) = guard.as_ref() {
                return Ok(c.index.clone());
            }
            Err(ClientError::Unavailable)
        }
    }

    /// Force-refresh. Pre-populates the in-memory cache from the on-disk
    /// cache on first call (so the daemon can start cold and still serve
    /// the prior index without a successful network fetch).
    ///
    /// # Errors
    /// Returns a `ClientError` on network failure, I/O error, or JSON parse error.
    /// Returns `ClientError::Unavailable` if the server answers with an unsolicited
    /// `304 Not Modified` and there is no cached index to fall back on.
    pub async fn refresh(&self) -> Result<Index, ClientError> {
        // Load from disk if memory is empty.
        let etag = {
            let need_load = self.state.read().is_none();
            if need_load {
                if let Some((idx, etag)) = self.load_from_disk()? {
                    self.state.write().replace(Cached {
                        index: idx,
                        etag: etag.clone(),
                        fetched_at: SystemTime::UNIX_EPOCH,
                    });
                    etag
                } else {
                    None
                }
            } else {
                self.state.read().as_ref().and_then(|c| c.etag.clone())
            }
        };

        let mut req = self.http.get(&self.url);
        if let Some(e) = &etag {
            req = req.header("If-None-Match", e);
        }
        let resp = req.send().await?;

        if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
            // A 304 is only valid as the answer to our conditional request, so a
            // cached index should exist to refresh. If it doesn't — a misbehaving
            // proxy replying 304 when we sent no `If-None-Match` — treat it as
            // unavailable rather than unwrapping `None` and panicking the daemon.
            let mut guard = self.state.write();
            let Some(c) = guard.as_mut() else {
                return Err(ClientError::Unavailable);
            };
            c.fetched_at = SystemTime::now();
            return Ok(c.index.clone());
        }

        let resp = resp.error_for_status()?;
        let new_etag = resp
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let bytes = resp.bytes().await?;
        let mut index: Index = serde_json::from_slice(&bytes)?;
        retain_safe_backends(&mut index);
        warn_if_client_too_old(&index);
        let cached = Cached {
            index: index.clone(),
            etag: new_etag,
            fetched_at: SystemTime::now(),
        };
        self.state.write().replace(cached.clone());
        self.persist(&cached, &bytes)?;
        Ok(index)
    }

    fn load_from_disk(&self) -> Result<Option<(Index, Option<String>)>, ClientError> {
        if !self.cache_path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&self.cache_path)?;
        let file: CacheFile = serde_json::from_slice(&bytes)?;
        let mut index: Index = serde_json::from_value(file.index)?;
        retain_safe_backends(&mut index);
        warn_if_client_too_old(&index);
        Ok(Some((index, file.etag)))
    }

    fn persist(&self, c: &Cached, body: &[u8]) -> Result<(), ClientError> {
        let file = CacheFile {
            etag: c.etag.clone(),
            fetched_at_secs: c
                .fetched_at
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            index: serde_json::from_slice(body)?,
        };
        super_stt_registry_types::fs::write_atomic(&self.cache_path, &serde_json::to_vec(&file)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fixture_index() -> &'static str {
        r#"{"schema_version":1,"generated_at":"now","min_client":"0.0.0","backends":[]}"#
    }

    #[tokio::test]
    async fn fetches_and_caches() {
        crate::install_crypto_provider();
        let mut s = mockito::Server::new_async().await;
        s.mock("GET", "/idx.json")
            .with_status(200)
            .with_header("etag", "\"abc\"")
            .with_body(fixture_index())
            .create_async()
            .await;
        let dir = tempdir().unwrap();
        let c = Client::new(
            format!("{}/idx.json", s.url()),
            dir.path().join("c.json"),
            DEFAULT_TTL,
        );
        let idx = c.refresh().await.unwrap();
        assert_eq!(idx.schema_version, 1);
        assert!(dir.path().join("c.json").exists());
    }

    #[tokio::test]
    async fn unsolicited_304_without_cache_is_unavailable_not_panic() {
        // A misbehaving proxy can answer 304 even though we sent no
        // If-None-Match (fresh client, no disk cache). Must surface as
        // Unavailable, not panic on an empty in-memory cache.
        crate::install_crypto_provider();
        let mut s = mockito::Server::new_async().await;
        s.mock("GET", "/idx.json")
            .with_status(304)
            .create_async()
            .await;
        let dir = tempdir().unwrap();
        let c = Client::new(
            format!("{}/idx.json", s.url()),
            dir.path().join("c.json"),
            DEFAULT_TTL,
        );
        assert!(matches!(c.refresh().await, Err(ClientError::Unavailable)));
    }

    #[tokio::test]
    async fn returns_cache_when_network_fails() {
        crate::install_crypto_provider();
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("c.json");
        std::fs::write(
            &cache_path,
            format!(
                r#"{{"etag":null,"fetched_at_secs":0,"index":{}}}"#,
                fixture_index()
            ),
        )
        .unwrap();
        let c = Client::new("http://127.0.0.1:1/never", cache_path, DEFAULT_TTL);
        let idx = c.get().await.unwrap();
        assert_eq!(idx.schema_version, 1);
    }
}
