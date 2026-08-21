// SPDX-License-Identifier: GPL-3.0-only
//! Streaming downloads for the release tarball and its `SHA256SUMS` listing.

use std::path::Path;

use crate::errors::InstallError;

/// Stream `url` to `dest`, reporting `(bytes_done, bytes_total)` after every
/// chunk (`bytes_total` is `0` when the server does not send a
/// `Content-Length`). Uses [`super_stt_forge::http::download_client`]'s 1 h
/// timeout, appropriate for a multi-hundred-MB release tarball.
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
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| InstallError::DownloadFailed(e.to_string()))?
    {
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .map_err(|e| InstallError::DownloadFailed(e.to_string()))?;
        done += chunk.len() as u64;
        on_progress(done, total);
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
        let mut last = (0, 0);
        download_to_file(&format!("{}/blob", s.url()), &dest, |d, t| last = (d, t))
            .await
            .unwrap();
        // Full content, not just length: guards against a last-chunk write
        // that was accepted into tokio's internal buffer but never actually
        // flushed to disk before the file was dropped (a real, if
        // intermittent, tokio::fs::File pitfall — see the `flush` call in
        // `download_to_file`).
        assert_eq!(std::fs::read(&dest).unwrap(), vec![7u8; 4096]);
        assert_eq!(last.0, 4096);
    }
}
