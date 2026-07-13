// SPDX-License-Identifier: GPL-3.0-only
//! Per-asset validation + SHA-256 over streamed downloads.
//!
//! A subprocess variant's archive is one release asset (`file`) or several
//! (`parts`, concatenated in order). Each release asset is capped at 2 GiB (the
//! GitHub limit) and pinned independently; the reassembled tar is validated by
//! streaming the parts in order, so a multi-GB archive is never held in memory.

use std::io::Read;
use std::path::{Path, PathBuf};

use ring::digest::{Context, SHA256};
use super_stt_registry_types::verify::{tar_budget_step, tar_entry_unsafe_reason, unpack_cap};
use thiserror::Error;

/// Ceiling for a single release asset — a `file` or one part of a multi-part
/// archive. Matches GitHub's 2 GiB per-asset release limit; a larger archive
/// must be split into `parts`.
pub const MAX_ASSET_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Ceiling for a `backend.toml` manifest asset — manifests are tiny; this only
/// bounds a hostile or mistaken upload.
pub const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6d];

#[derive(Debug, Error)]
pub enum AssetError {
    #[error("asset `{0}` is missing from the release")]
    Missing(String),
    #[error("asset `{file}` size {size} exceeds {MAX_ASSET_BYTES}")]
    TooLarge { file: String, size: u64 },
    #[error("asset `{0}` does not start with the wasm32 magic header")]
    NotWasm(String),
    #[error("tarball `{file}` contains escape entry `{entry}`")]
    TarEscape { file: String, entry: String },
    #[error("tarball `{file}` breaches the unpack budget: {reason}")]
    TarBudget { file: String, reason: String },
    #[error("tarball `{file}` does not contain `bin/{entrypoint}`")]
    TarMissingEntrypoint { file: String, entrypoint: String },
    #[error(transparent)]
    Http(#[from] anyhow::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Resolve a declared asset's URL via the release manifest, refusing if missing
/// or larger than [`MAX_ASSET_BYTES`].
pub fn resolve_url(
    file: &str,
    release_assets: &[super_stt_forge::ReleaseAsset],
) -> Result<(String, u64), AssetError> {
    let a = release_assets
        .iter()
        .find(|a| a.name == file)
        .ok_or_else(|| AssetError::Missing(file.into()))?;
    if a.size > MAX_ASSET_BYTES {
        return Err(AssetError::TooLarge {
            file: file.into(),
            size: a.size,
        });
    }
    Ok((a.download_url.clone(), a.size))
}

/// Stream a wasm component, verify the `wasm32` magic header, and return its
/// SHA-256. Components are small, so this does not touch disk.
pub async fn fetch_wasm_and_hash(
    http: &reqwest::Client,
    url: &str,
    file: &str,
) -> Result<String, AssetError> {
    use futures::StreamExt;
    let mut resp = http
        .get(url)
        .send()
        .await
        .map_err(|e| AssetError::Http(e.into()))?
        .error_for_status()
        .map_err(|e| AssetError::Http(e.into()))?
        .bytes_stream();
    let mut ctx = Context::new(&SHA256);
    let mut first_chunk: Vec<u8> = Vec::new();
    let mut total: u64 = 0;
    while let Some(chunk) = resp.next().await {
        let chunk = chunk.map_err(|e| AssetError::Http(e.into()))?;
        total += chunk.len() as u64;
        if total > MAX_ASSET_BYTES {
            return Err(AssetError::TooLarge {
                file: file.into(),
                size: total,
            });
        }
        ctx.update(&chunk);
        if first_chunk.len() < 4 {
            let need = 4 - first_chunk.len();
            first_chunk.extend_from_slice(&chunk[..need.min(chunk.len())]);
        }
    }
    if first_chunk != WASM_MAGIC {
        return Err(AssetError::NotWasm(file.into()));
    }
    Ok(hex::encode(ctx.finish().as_ref()))
}

/// Stream a release asset (a `file`, or one part of a multi-part archive) to
/// `dest`, computing its size and SHA-256 and enforcing [`MAX_ASSET_BYTES`].
/// Writing to disk keeps a multi-GB part out of memory; the caller validates
/// the reassembled archive with [`validate_subprocess_parts`].
pub async fn download_to_file(
    http: &reqwest::Client,
    url: &str,
    file: &str,
    dest: &Path,
) -> Result<(u64, String), AssetError> {
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;
    let mut resp = http
        .get(url)
        .send()
        .await
        .map_err(|e| AssetError::Http(e.into()))?
        .error_for_status()
        .map_err(|e| AssetError::Http(e.into()))?
        .bytes_stream();
    let mut out = tokio::fs::File::create(dest).await?;
    let mut ctx = Context::new(&SHA256);
    let mut total: u64 = 0;
    while let Some(chunk) = resp.next().await {
        let chunk = chunk.map_err(|e| AssetError::Http(e.into()))?;
        total += chunk.len() as u64;
        if total > MAX_ASSET_BYTES {
            return Err(AssetError::TooLarge {
                file: file.into(),
                size: total,
            });
        }
        ctx.update(&chunk);
        out.write_all(&chunk).await?;
    }
    out.flush().await?;
    Ok((total, hex::encode(ctx.finish().as_ref())))
}

/// Download the `backend.toml` manifest asset, returning its raw bytes and
/// SHA-256. The indexer parses + validates these exact bytes and records the
/// hash so the daemon can verify it installs the same bytes. Capped at
/// [`MAX_MANIFEST_BYTES`].
pub async fn fetch_manifest_asset(
    http: &reqwest::Client,
    url: &str,
) -> Result<(Vec<u8>, String), AssetError> {
    use futures::StreamExt;
    let mut resp = http
        .get(url)
        .send()
        .await
        .map_err(|e| AssetError::Http(e.into()))?
        .error_for_status()
        .map_err(|e| AssetError::Http(e.into()))?
        .bytes_stream();
    let mut ctx = Context::new(&SHA256);
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.next().await {
        let chunk = chunk.map_err(|e| AssetError::Http(e.into()))?;
        if buf.len() as u64 + chunk.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(AssetError::TooLarge {
                file: "backend.toml".into(),
                size: buf.len() as u64 + chunk.len() as u64,
            });
        }
        ctx.update(&chunk);
        buf.extend_from_slice(&chunk);
    }
    Ok((buf, hex::encode(ctx.finish().as_ref())))
}

/// Validate that the reassembled subprocess archive — the in-order
/// concatenation of `parts` (one entry for a single-file asset) — is a
/// `.tar.gz` containing `bin/<entrypoint>` with no path-escaping or symlink
/// entries. Streams the parts through the decoder so a multi-GB archive is
/// never held in memory.
pub fn validate_subprocess_parts(
    parts: &[PathBuf],
    file: &str,
    entrypoint: &str,
) -> Result<(), AssetError> {
    // Compressed size = sum of the part files; the unpack budget scales with it
    // so the indexer applies the same zip-bomb ceiling the daemon enforces at
    // install (a green publish that would fail every install is rejected here).
    let mut compressed: u64 = 0;
    let mut chained: Box<dyn Read> = Box::new(std::io::empty());
    for p in parts {
        compressed = compressed.saturating_add(std::fs::metadata(p)?.len());
        let f = std::fs::File::open(p)?;
        chained = Box::new(chained.chain(f));
    }
    validate_tarball_read(file, entrypoint, chained, unpack_cap(compressed))
}

/// The tar-content checks, over any reader: rejects path-traversal and symlink
/// entries (the shared [`tar_entry_unsafe_reason`] predicate), enforces the
/// per-entry and total unpack budgets ([`tar_budget_step`], `total_cap` from
/// [`unpack_cap`]), and requires `bin/<entrypoint>` (or the bare entrypoint
/// path). The safety predicate and budgets are the exact ones the daemon
/// applies at install.
fn validate_tarball_read<R: Read>(
    file: &str,
    entrypoint: &str,
    r: R,
    total_cap: u64,
) -> Result<(), AssetError> {
    let gz = flate2::read::GzDecoder::new(r);
    let mut archive = tar::Archive::new(gz);
    let mut found_entrypoint = false;
    let mut total: u64 = 0;
    for entry in archive.entries()? {
        let entry = entry?;
        let path = entry.path()?;
        let s = path.to_string_lossy();
        if let Some(reason) = tar_entry_unsafe_reason(&s, entry.header().entry_type().is_symlink())
        {
            return Err(AssetError::TarEscape {
                file: file.into(),
                entry: reason,
            });
        }
        total = tar_budget_step(entry.size(), total, total_cap).map_err(|reason| {
            AssetError::TarBudget {
                file: file.into(),
                reason,
            }
        })?;
        // Accept the entrypoint at its declared path (e.g. `bin/qwen3-asr`),
        // or the legacy bare-name-under-bin form (`bin/<entrypoint>`).
        if s == entrypoint || s == format!("bin/{entrypoint}") {
            found_entrypoint = true;
        }
    }
    if !found_entrypoint {
        return Err(AssetError::TarMissingEntrypoint {
            file: file.into(),
            entrypoint: entrypoint.into(),
        });
    }
    Ok(())
}

#[cfg(test)]
fn validate_tarball(file: &str, entrypoint: &str, bytes: &[u8]) -> Result<(), AssetError> {
    validate_tarball_read(file, entrypoint, bytes, unpack_cap(bytes.len() as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    fn make_tarball<F: FnOnce(&mut tar::Builder<GzEncoder<Vec<u8>>>)>(f: F) -> Vec<u8> {
        let gz = GzEncoder::new(Vec::new(), Compression::default());
        let mut tb = tar::Builder::new(gz);
        f(&mut tb);
        tb.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn accepts_tarball_with_bin_entrypoint() {
        let bytes = make_tarball(|tb| {
            let mut h = tar::Header::new_gnu();
            h.set_size(3);
            h.set_mode(0o755);
            h.set_cksum();
            tb.append_data(&mut h, "bin/voxtral", &b"abc"[..]).unwrap();
        });
        validate_tarball("v.tar.gz", "voxtral", &bytes).unwrap();
    }

    #[test]
    fn accepts_tarball_with_path_entrypoint() {
        let bytes = make_tarball(|tb| {
            let mut h = tar::Header::new_gnu();
            h.set_size(3);
            h.set_mode(0o755);
            h.set_cksum();
            tb.append_data(&mut h, "bin/qwen3-asr", &b"abc"[..])
                .unwrap();
        });
        validate_tarball("q.tar.gz", "bin/qwen3-asr", &bytes).unwrap();
    }

    #[test]
    fn rejects_output_over_the_unpack_budget_at_publish() {
        // The unpack budget is now enforced at publish (it wasn't before), so a
        // tarball whose unpacked output exceeds the cap is rejected here rather
        // than passing publish and failing every install. Drive the validator
        // with a deliberately tiny `total_cap` so the check trips on a small
        // archive — no need to materialize gigabytes.
        let bytes = make_tarball(|tb| {
            let body = vec![0u8; 200];
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o755);
            h.set_cksum();
            tb.append_data(&mut h, "bin/voxtral", &body[..]).unwrap();
        });
        // total_cap = 100 < the 200-byte entry.
        let err = validate_tarball_read("v.tar.gz", "voxtral", &bytes[..], 100).unwrap_err();
        assert!(
            matches!(err, AssetError::TarBudget { .. }),
            "expected TarBudget, got {err:?}"
        );
        // The same archive validates when the cap is generous.
        validate_tarball_read("v.tar.gz", "voxtral", &bytes[..], 10_000).unwrap();
    }

    #[test]
    fn rejects_tarball_without_entrypoint() {
        let bytes = make_tarball(|tb| {
            let mut h = tar::Header::new_gnu();
            h.set_size(3);
            h.set_mode(0o644);
            h.set_cksum();
            tb.append_data(&mut h, "README", &b"abc"[..]).unwrap();
        });
        let err = validate_tarball("v.tar.gz", "voxtral", &bytes).unwrap_err();
        assert!(matches!(err, AssetError::TarMissingEntrypoint { .. }));
    }

    /// Build a minimal tar.gz from raw bytes, bypassing `tar::Builder`'s path
    /// sanitisation so we can embed `../escape` directly.
    fn make_raw_tar_gz_with_path(path: &str) -> Vec<u8> {
        // POSIX ustar header: 512 bytes.
        let mut header = [0u8; 512];
        let name = path.as_bytes();
        let len = name.len().min(100);
        header[..len].copy_from_slice(&name[..len]);
        // file mode
        header[100..108].copy_from_slice(b"0000644\0");
        // uid / gid
        header[108..116].copy_from_slice(b"0000000\0");
        header[116..124].copy_from_slice(b"0000000\0");
        // size = 0
        header[124..136].copy_from_slice(b"00000000000\0");
        // mtime
        header[136..148].copy_from_slice(b"00000000000\0");
        // type flag: regular file
        header[156] = b'0';
        // ustar magic
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        // Compute checksum (sum of bytes with checksum field as spaces).
        header[148..156].copy_from_slice(b"        ");
        let sum: u32 = header.iter().map(|&b| u32::from(b)).sum();
        let cksum = format!("{sum:06o}\0 ");
        header[148..156].copy_from_slice(cksum.as_bytes());
        // Two 512-byte zero blocks mark end-of-archive.
        let mut tar_bytes = Vec::new();
        tar_bytes.extend_from_slice(&header);
        tar_bytes.extend_from_slice(&[0u8; 1024]);
        // Compress.
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut enc = gz;
        enc.write_all(&tar_bytes).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn rejects_path_traversal() {
        let bytes = make_raw_tar_gz_with_path("../escape");
        let err = validate_tarball("v.tar.gz", "voxtral", &bytes).unwrap_err();
        assert!(matches!(err, AssetError::TarEscape { .. }));
    }

    /// A tarball split into parts validates when the parts are concatenated in
    /// order — the multi-part reassembly path.
    #[test]
    fn validates_tarball_reassembled_from_parts() {
        let bytes = make_tarball(|tb| {
            let mut h = tar::Header::new_gnu();
            h.set_size(3);
            h.set_mode(0o755);
            h.set_cksum();
            tb.append_data(&mut h, "bin/qwen3-asr", &b"abc"[..])
                .unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let mid = bytes.len() / 2;
        let p0 = dir.path().join("a.part00");
        let p1 = dir.path().join("a.part01");
        std::fs::write(&p0, &bytes[..mid]).unwrap();
        std::fs::write(&p1, &bytes[mid..]).unwrap();
        validate_subprocess_parts(&[p0, p1], "q.tar.gz", "bin/qwen3-asr").unwrap();
    }

    /// Parts concatenated out of order do not reassemble into a valid archive.
    #[test]
    fn rejects_parts_in_wrong_order() {
        let bytes = make_tarball(|tb| {
            let mut h = tar::Header::new_gnu();
            h.set_size(3);
            h.set_mode(0o755);
            h.set_cksum();
            tb.append_data(&mut h, "bin/qwen3-asr", &b"abc"[..])
                .unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let mid = bytes.len() / 2;
        let p0 = dir.path().join("a.part00");
        let p1 = dir.path().join("a.part01");
        std::fs::write(&p0, &bytes[..mid]).unwrap();
        std::fs::write(&p1, &bytes[mid..]).unwrap();
        // Swapped order → corrupt gzip → an error (not a clean archive).
        assert!(validate_subprocess_parts(&[p1, p0], "q.tar.gz", "bin/qwen3-asr").is_err());
    }
}
