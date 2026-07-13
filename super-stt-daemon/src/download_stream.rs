// SPDX-License-Identifier: GPL-3.0-only
//! One streaming-download loop shared by the registry install pipeline
//! (`registry/install.rs`) and the model-file downloader
//! (`stt_models/download.rs`).
//!
//! [`stream_body_to_writer`] streams a response body into a writer, hashing as
//! it goes, enforcing an optional byte cap and an optional cancellation
//! predicate, and reporting per-chunk progress. The caller owns everything
//! around it — the file's create/rename/fsync, the SHA-256 verification, and
//! mapping [`StreamError`] onto its own error type — because those differ across
//! the single-file, multi-part-append, and cancellable-model-download callers.

use futures::StreamExt;
use ring::digest::{Context, SHA256};
use tokio::io::{AsyncWrite, AsyncWriteExt};

/// Failure while streaming a response body to disk. Callers map this onto their
/// own error type (`PipelineError`, `anyhow::Error`).
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("download exceeds {limit} bytes")]
    TooLarge { limit: u64 },
    #[error("download cancelled")]
    Cancelled,
}

/// Stream `resp`'s body into `writer`, returning `(bytes_written, sha256_hex)`.
///
/// - `cap`: abort once more than this many bytes have arrived (before the disk
///   fills), `None` for no ceiling. The over-cap chunk is not written.
/// - `should_cancel`: checked before each chunk; `true` aborts with
///   [`StreamError::Cancelled`]. Pass `|| false` when cancellation doesn't apply.
/// - `on_chunk`: called with each written chunk's length (a delta) for progress;
///   the caller accumulates if it needs a running total.
///
/// On success the writer is flushed — but **not** `fsync`'d and **not** renamed
/// or removed: durability and the file's fate are the caller's. The SHA-256 is
/// always computed and returned; the caller verifies it when a pin exists.
///
/// # Errors
/// [`StreamError`] on a transport error, an I/O write error, exceeding `cap`, or
/// cancellation.
pub async fn stream_body_to_writer<W, C, P>(
    resp: reqwest::Response,
    writer: &mut W,
    cap: Option<u64>,
    should_cancel: C,
    mut on_chunk: P,
) -> Result<(u64, String), StreamError>
where
    W: AsyncWrite + Unpin,
    C: Fn() -> bool,
    P: FnMut(u64),
{
    let mut hasher = Context::new(&SHA256);
    let mut stream = resp.bytes_stream();
    let mut bytes_done: u64 = 0;
    while let Some(chunk) = stream.next().await {
        if should_cancel() {
            return Err(StreamError::Cancelled);
        }
        let chunk = chunk?;
        bytes_done += chunk.len() as u64;
        if let Some(limit) = cap
            && bytes_done > limit
        {
            return Err(StreamError::TooLarge { limit });
        }
        writer.write_all(&chunk).await?;
        hasher.update(&chunk);
        on_chunk(chunk.len() as u64);
    }
    writer.flush().await?;
    Ok((bytes_done, hex::encode(hasher.finish().as_ref())))
}
