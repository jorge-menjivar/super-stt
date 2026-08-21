// SPDX-License-Identifier: GPL-3.0-only
//! Streaming downloads for the release tarball and its `SHA256SUMS` listing.

use std::path::Path;

use crate::errors::InstallError;

/// Minimum byte delta between `on_progress` emissions, so a fast transfer
/// doesn't flood the caller (JSON progress lines on the app's stdin pipe)
/// with one message per `reqwest` chunk (~16 KiB — tens of thousands of
/// lines on a real multi-hundred-MB tarball). Mirrors
/// `super-stt-app::core::app::updater::PROGRESS_THROTTLE_BYTES` — keep both
/// in step.
const PROGRESS_THROTTLE_BYTES: u64 = 256 * 1024;

/// Stream `url` to `dest`, reporting throttled `(bytes_done, bytes_total)`
/// progress (`bytes_total` is `0` when the server does not send a
/// `Content-Length`): emitted when at least [`PROGRESS_THROTTLE_BYTES`] have
/// landed since the last emission, OR the transfer just completed
/// (`bytes_done == bytes_total`) — that final emission is unconditional, so
/// even a payload smaller than the threshold still reports at least once.
/// Uses [`super_stt_forge::http::download_client`]'s 1 h timeout, appropriate
/// for a multi-hundred-MB release tarball.
///
/// # Errors
/// [`InstallError::DownloadFailed`] on a network failure, a non-2xx response,
/// or a local I/O error creating/writing `dest`.
pub async fn download_to_file(
    url: &str,
    dest: &Path,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<(), InstallError> {
    let client = super_stt_forge::http::download_client();
    let mut resp = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| InstallError::DownloadFailed(format!("{url}: {e}")))?;
    let total = resp.content_length().unwrap_or(0);
    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|e| InstallError::DownloadFailed(format!("create {}: {e}", dest.display())))?;
    let mut done: u64 = 0;
    let mut last_reported: u64 = 0;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| InstallError::DownloadFailed(e.to_string()))?
    {
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .map_err(|e| InstallError::DownloadFailed(e.to_string()))?;
        done += chunk.len() as u64;
        if done.saturating_sub(last_reported) >= PROGRESS_THROTTLE_BYTES || done == total {
            last_reported = done;
            on_progress(done, total);
        }
    }
    // `tokio::fs::File::poll_write` returns as soon as bytes are copied into
    // its internal buffer, *before* the spawned blocking write to the OS
    // completes — dropping the file without waiting for that would risk the
    // last chunk not actually landing on disk (an intermittent, silent
    // truncation the immediately-following checksum verification would
    // report as a mismatch). `flush` drives that pending write to
    // completion and surfaces any I/O error from it.
    tokio::io::AsyncWriteExt::flush(&mut file)
        .await
        .map_err(|e| InstallError::DownloadFailed(format!("flush {}: {e}", dest.display())))?;
    Ok(())
}

/// Fetch `url` as a UTF-8 string (used for the small `SHA256SUMS` listing),
/// aborting as soon as the body would exceed `max_bytes`.
///
/// # Errors
/// [`InstallError::DownloadFailed`] on a network failure, a non-2xx response,
/// a body over `max_bytes`, or invalid UTF-8.
pub async fn download_string(url: &str, max_bytes: u64) -> Result<String, InstallError> {
    let client = super_stt_forge::http::download_client();
    let mut resp = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| InstallError::DownloadFailed(format!("{url}: {e}")))?;
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| InstallError::DownloadFailed(e.to_string()))?
    {
        if body.len() as u64 + chunk.len() as u64 > max_bytes {
            return Err(InstallError::DownloadFailed(format!(
                "{url}: response exceeded {max_bytes} bytes"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).map_err(|e| InstallError::DownloadFailed(format!("{url}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn download_to_file_streams_and_reports() {
        super_stt_forge::install_crypto_provider();
        let mut s = mockito::Server::new_async().await;
        s.mock("GET", "/blob")
            .with_status(200)
            .with_body(vec![7u8; 4096])
            .create_async()
            .await;
        let dir = std::env::temp_dir().join(format!("sstt-install-dl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("blob");
        let mut calls: Vec<(u64, u64)> = Vec::new();
        download_to_file(&format!("{}/blob", s.url()), &dest, |d, t| {
            calls.push((d, t))
        })
        .await
        .unwrap();
        // Full content, not just length: guards against a last-chunk write
        // that was accepted into tokio's internal buffer but never actually
        // flushed to disk before the file was dropped (a real, if
        // intermittent, tokio::fs::File pitfall — see the `flush` call in
        // `download_to_file`).
        assert_eq!(std::fs::read(&dest).unwrap(), vec![7u8; 4096]);
        // A body well under the throttle threshold still reports exactly
        // once — the unconditional `done == total` final emission — so a
        // tiny payload (like the hermetic e2e test's fixture tarball) still
        // gets at least one progress event.
        assert_eq!(calls, vec![(4096, 4096)]);
    }

    #[tokio::test]
    async fn download_to_file_throttles_progress_on_a_large_body() {
        super_stt_forge::install_crypto_provider();
        let size: usize = 3 * 256 * 1024; // several times PROGRESS_THROTTLE_BYTES
        let mut s = mockito::Server::new_async().await;
        s.mock("GET", "/blob")
            .with_status(200)
            .with_body(vec![9u8; size])
            .create_async()
            .await;
        let dir =
            std::env::temp_dir().join(format!("sstt-install-dl-throttle-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("blob");
        let mut calls: Vec<(u64, u64)> = Vec::new();
        download_to_file(&format!("{}/blob", s.url()), &dest, |d, t| {
            calls.push((d, t))
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap().len(), size);
        // A body several times the throttle threshold emits more than once
        // (unlike the tiny-body case above) but far fewer times than one
        // emission per ~16 KiB `reqwest` chunk — the whole point of the
        // throttle.
        assert!(
            calls.len() > 1 && calls.len() < 20,
            "expected a small handful of throttled emissions, got {}: {calls:?}",
            calls.len()
        );
        // Every emission before the last must have been at least
        // PROGRESS_THROTTLE_BYTES past the previous one.
        for window in calls.windows(2) {
            assert!(
                window[1].0 - window[0].0 >= PROGRESS_THROTTLE_BYTES || window[1].0 == size as u64
            );
        }
        // The final emission always reports the full total, whether or not
        // it happened to also cross the throttle threshold.
        assert_eq!(*calls.last().unwrap(), (size as u64, size as u64));
    }
}
