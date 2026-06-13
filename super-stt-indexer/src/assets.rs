// SPDX-License-Identifier: GPL-3.0-only
//! Per-asset validation + SHA-256 over streamed downloads.

use ring::digest::{Context, SHA256};
use thiserror::Error;

pub const MAX_ASSET_BYTES: u64 = 200 * 1024 * 1024;
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
    #[error("tarball `{file}` does not contain `bin/{entrypoint}`")]
    TarMissingEntrypoint { file: String, entrypoint: String },
    #[error(transparent)]
    Http(#[from] anyhow::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Resolve a declared asset's URL via the release manifest, refusing if missing.
pub fn resolve_url(
    file: &str,
    release_assets: &[crate::github::ReleaseAsset],
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
    Ok((a.browser_download_url.clone(), a.size))
}

/// Stream the asset, compute SHA-256, and dispatch validation based on kind.
pub async fn fetch_and_validate(
    http: &reqwest::Client,
    url: &str,
    expected: AssetExpect<'_>,
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
    let mut buf: Vec<u8> = Vec::new();
    let mut first_chunk: Vec<u8> = Vec::new();
    let mut total_streamed: u64 = 0;
    while let Some(chunk) = resp.next().await {
        let chunk = chunk.map_err(|e| AssetError::Http(e.into()))?;
        total_streamed += chunk.len() as u64;
        if total_streamed > MAX_ASSET_BYTES {
            return Err(AssetError::TooLarge {
                file: expected.file().into(),
                size: total_streamed,
            });
        }
        ctx.update(&chunk);
        if first_chunk.len() < 4 {
            let need = 4 - first_chunk.len();
            first_chunk.extend_from_slice(&chunk[..need.min(chunk.len())]);
        }
        if matches!(expected, AssetExpect::Subprocess { .. }) {
            buf.extend_from_slice(&chunk);
        }
    }
    match expected {
        AssetExpect::Wasm { file } => {
            if first_chunk != WASM_MAGIC {
                return Err(AssetError::NotWasm(file.into()));
            }
        }
        AssetExpect::Subprocess { file, entrypoint } => {
            validate_tarball(file, entrypoint, &buf)?;
        }
    }
    Ok(hex::encode(ctx.finish().as_ref()))
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

pub enum AssetExpect<'a> {
    Wasm { file: &'a str },
    Subprocess { file: &'a str, entrypoint: &'a str },
}

impl AssetExpect<'_> {
    fn file(&self) -> &str {
        match self {
            AssetExpect::Wasm { file } | AssetExpect::Subprocess { file, .. } => file,
        }
    }
}

fn validate_tarball(file: &str, entrypoint: &str, bytes: &[u8]) -> Result<(), AssetError> {
    let gz = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);
    let mut found_entrypoint = false;
    for entry in archive.entries()? {
        let entry = entry?;
        let path = entry.path()?;
        let s = path.to_string_lossy();
        if s.starts_with('/') || s.contains("..") {
            return Err(AssetError::TarEscape {
                file: file.into(),
                entry: s.into(),
            });
        }
        if entry.header().entry_type().is_symlink() {
            // Reject symlinks outright; the daemon's installer also rejects.
            return Err(AssetError::TarEscape {
                file: file.into(),
                entry: s.into(),
            });
        }
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
}
