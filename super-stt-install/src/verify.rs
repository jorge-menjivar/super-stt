// SPDX-License-Identifier: GPL-3.0-only
//! SHA-256 checksum verification for downloaded tarballs against a
//! `SHA256SUMS` listing.

use std::io::Read;
use std::path::Path;

use crate::errors::InstallError;

/// Read a `SHA256SUMS`-format listing into `(hex_digest, filename)` pairs.
///
/// Tolerates both the coreutils `sha256sum` text-mode (`<hex>  <filename>`,
/// two spaces) and binary-mode (`<hex> *<filename>`) line shapes, and a
/// `./`-prefixed filename (`<hex>  ./<filename>`).
#[must_use]
pub fn parse_sha256sums(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let mut parts = line.splitn(2, char::is_whitespace);
            let hex = parts.next()?.to_string();
            let rest = parts.next()?.trim_start();
            let filename = rest.strip_prefix('*').unwrap_or(rest);
            let filename = filename.strip_prefix("./").unwrap_or(filename);
            Some((hex, filename.to_string()))
        })
        .collect()
}

/// Compute the SHA-256 digest of the file at `path`, streaming it in 1 MiB
/// reads so a multi-hundred-MB tarball is never fully buffered in memory.
///
/// # Errors
/// [`InstallError::ChecksumMismatch`] if `path` cannot be opened or read.
pub fn file_sha256_hex(path: &Path) -> Result<String, InstallError> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| InstallError::ChecksumMismatch(format!("{}: {e}", path.display())))?;
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| InstallError::ChecksumMismatch(format!("{}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
    }
    Ok(hex::encode(ctx.finish().as_ref()))
}

/// Verify that the file at `path` matches the digest `sums_text` (a
/// `SHA256SUMS` listing) records for `filename`.
///
/// # Errors
/// [`InstallError::ChecksumMismatch`] when `filename` is not listed in
/// `sums_text`, the digest does not match, or `path` cannot be hashed.
pub fn verify_file(path: &Path, filename: &str, sums_text: &str) -> Result<(), InstallError> {
    let sums = parse_sha256sums(sums_text);
    let Some((expected, _)) = sums.iter().find(|(_, name)| name == filename) else {
        return Err(InstallError::ChecksumMismatch(format!(
            "{filename}: not listed in SHA256SUMS"
        )));
    };
    let actual = file_sha256_hex(path)?;
    if super_stt_registry_types::verify::sha256_matches(&actual, expected) {
        Ok(())
    } else {
        Err(InstallError::ChecksumMismatch(filename.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sha256sums_variants() {
        let text =
            "abc123  super-stt-x.tar.gz\ndef456 *binary-mode-file\n789aaa  ./dotslash-file\n";
        let sums = parse_sha256sums(text);
        assert_eq!(sums.len(), 3);
        assert_eq!(sums[0], ("abc123".into(), "super-stt-x.tar.gz".into()));
        assert_eq!(sums[1].1, "binary-mode-file");
        assert_eq!(sums[2].1, "dotslash-file");
    }

    #[test]
    fn verify_file_matches_known_vector() {
        // sha256("hello world\n") — a standard test vector.
        let dir = std::env::temp_dir().join(format!("sstt-install-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("hello.txt");
        std::fs::write(&f, "hello world\n").unwrap();
        let sums = "a948904f2f0f479b8f8197694b30184b0d2ed1c1cd2a1ec0fb85d299a192a447  hello.txt\n";
        assert!(verify_file(&f, "hello.txt", sums).is_ok());
        assert!(matches!(
            verify_file(
                &f,
                "hello.txt",
                "0000000000000000000000000000000000000000000000000000000000000000  hello.txt\n"
            ),
            Err(crate::errors::InstallError::ChecksumMismatch(_))
        ));
        assert!(matches!(
            verify_file(&f, "unlisted.txt", sums),
            Err(crate::errors::InstallError::ChecksumMismatch(_))
        ));
    }
}
