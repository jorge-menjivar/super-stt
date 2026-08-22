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
/// progress: emitted when at least [`PROGRESS_THROTTLE_BYTES`] have landed
/// since the last emission, or the transfer just completed
/// (`bytes_done == bytes_total`). A final emission at the very end is always
/// guaranteed on top of that, even when neither condition ever fired during
/// the transfer — the case for any payload smaller than the throttle
/// threshold when the server sends no `Content-Length` (`bytes_total` would
/// otherwise stay `0` for the whole transfer, so `bytes_done == bytes_total`
/// never triggers): that guaranteed final event reports the actual bytes
/// downloaded as `bytes_total` too, so it always reads as "done" rather than
/// an unknown/zero total. Uses [`super_stt_forge::http::download_client`]'s
/// 1 h timeout, appropriate for a multi-hundred-MB release tarball.
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
    let mut any_progress = false;
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
            any_progress = true;
            on_progress(done, total);
        }
    }
    // F8: guarantee at least one emission even when nothing above ever
    // fired. The old `done == total` check alone can't do this when the
    // server sends no `Content-Length` (`total` stays `0` for the whole
    // transfer): `done == total` is then only ever true for a zero-byte
    // body, so any real payload under the throttle threshold silently
    // reported nothing at all. When the true total was never known, report
    // the actual bytes downloaded as the total too, so a consumer computing
    // a percentage sees a clean "done" rather than a `bytes_done > 0` over
    // an unknown/zero total.
    if !any_progress || last_reported != done {
        let final_total = if total == 0 { done } else { total };
        on_progress(done, final_total);
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

    #[tokio::test]
    async fn download_to_file_reports_final_progress_with_no_content_length() {
        // F8: when the server sends no `Content-Length`, `total` is 0 for
        // the whole transfer, so the old `done == total` check could never
        // be true after the first byte — a small body (under the throttle)
        // would silently emit NOTHING. There must always be at least one
        // final emission, reporting the real byte count actually
        // downloaded (not 0) as the total.
        super_stt_forge::install_crypto_provider();
        let mut s = mockito::Server::new_async().await;
        s.mock("GET", "/blob")
            // A chunked body (rather than `with_body`) is what actually
            // reproduces "no Content-Length" against `reqwest`: mockito
            // sets `Content-Length` automatically for `with_body`, but never
            // for a chunked response.
            .with_status(200)
            .with_chunked_body(|w| w.write_all(&[3u8; 1000]))
            .create_async()
            .await;
        let dir =
            std::env::temp_dir().join(format!("sstt-install-dl-nolen-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("blob");
        let mut calls: Vec<(u64, u64)> = Vec::new();
        download_to_file(&format!("{}/blob", s.url()), &dest, |d, t| {
            calls.push((d, t))
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap().len(), 1000);
        assert!(
            !calls.is_empty(),
            "a no-Content-Length response must still emit at least one progress event"
        );
        // The final emission reports the real bytes downloaded, not the
        // (unknown) Content-Length total.
        assert_eq!(*calls.last().unwrap(), (1000, 1000));
    }

    #[tokio::test]
    async fn download_to_file_maps_a_404_to_download_failed() {
        super_stt_forge::install_crypto_provider();
        let mut s = mockito::Server::new_async().await;
        s.mock("GET", "/missing")
            .with_status(404)
            .create_async()
            .await;
        let dir = std::env::temp_dir().join(format!("sstt-install-dl-404-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("blob");
        let err = download_to_file(&format!("{}/missing", s.url()), &dest, |_, _| {})
            .await
            .unwrap_err();
        assert!(matches!(err, InstallError::DownloadFailed(_)));
    }

    #[tokio::test]
    async fn download_to_file_maps_a_connection_refused_to_download_failed() {
        super_stt_forge::install_crypto_provider();
        // A loopback port nothing is listening on: `TcpListener::bind(0)`
        // then dropping it immediately frees the OS-assigned port while
        // keeping the attempt realistic (rather than a hardcoded port that
        // might legitimately be in use on the test host).
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let dir =
            std::env::temp_dir().join(format!("sstt-install-dl-refused-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("blob");
        let err = download_to_file(&format!("http://127.0.0.1:{port}/blob"), &dest, |_, _| {})
            .await
            .unwrap_err();
        assert!(matches!(err, InstallError::DownloadFailed(_)));
    }

    #[tokio::test]
    async fn download_string_returns_body_within_cap() {
        super_stt_forge::install_crypto_provider();
        let mut s = mockito::Server::new_async().await;
        s.mock("GET", "/sums")
            .with_status(200)
            .with_body("deadbeef  file.tar.gz\n")
            .create_async()
            .await;
        let body = download_string(&format!("{}/sums", s.url()), 1024)
            .await
            .unwrap();
        assert_eq!(body, "deadbeef  file.tar.gz\n");
    }

    #[tokio::test]
    async fn download_string_enforces_its_cap() {
        // A body larger than `max_bytes` must error rather than silently
        // truncating — a truncated `SHA256SUMS` could otherwise drop the
        // one line the caller actually needed to verify.
        super_stt_forge::install_crypto_provider();
        let mut s = mockito::Server::new_async().await;
        s.mock("GET", "/big")
            .with_status(200)
            .with_body(vec![b'a'; 2048])
            .create_async()
            .await;
        let err = download_string(&format!("{}/big", s.url()), 1024)
            .await
            .unwrap_err();
        assert!(matches!(err, InstallError::DownloadFailed(_)));
    }
}
