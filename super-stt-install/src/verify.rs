// SPDX-License-Identifier: GPL-3.0-only
//! SHA-256 checksum verification for downloaded tarballs against a
//! `SHA256SUMS` listing. Parsing and streaming file-hashing live in
//! `super_stt_registry_types::verify` (shared with the daemon's self-update
//! installer-checksum lookup — both must stay in lock-step); this module
//! composes them into the install-specific error type.

use std::path::Path;

use super_stt_registry_types::verify::{file_sha256_hex, parse_sha256sums, sha256_matches};

use crate::errors::InstallError;

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
    let actual = file_sha256_hex(path)
        .map_err(|e| InstallError::ChecksumMismatch(format!("{}: {e}", path.display())))?;
    if sha256_matches(&actual, expected) {
        Ok(())
    } else {
        Err(InstallError::ChecksumMismatch(filename.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn verify_file_accepts_an_uppercase_digest_in_the_sums_listing() {
        // The daemon's and app's own checksum lookups rely on this
        // case-insensitivity too (`sha256_matches`) — an `index.json`/manifest
        // pin may be upper- or mixed-case.
        let dir =
            std::env::temp_dir().join(format!("sstt-install-test-upper-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("hello.txt");
        std::fs::write(&f, "hello world\n").unwrap();
        let sums = "A948904F2F0F479B8F8197694B30184B0D2ED1C1CD2A1EC0FB85D299A192A447  hello.txt\n";
        assert!(verify_file(&f, "hello.txt", sums).is_ok());
    }

    #[test]
    fn verify_file_treats_a_malformed_digest_entry_as_not_listed() {
        // Wave 1's `parse_sha256sums` guard skips any entry whose digest
        // isn't exactly 64 ASCII hex chars, rather than yielding a truncated
        // or corrupted digest as if it were real. From `verify_file`'s side
        // that must surface as "not listed", not a checksum-mismatch bug
        // report about a digest nobody actually published.
        let dir = std::env::temp_dir().join(format!(
            "sstt-install-test-malformed-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("hello.txt");
        std::fs::write(&f, "hello world\n").unwrap();
        // Truncated digest (63 hex chars) — not shape-valid, so it's skipped
        // by `parse_sha256sums`, leaving `hello.txt` unlisted.
        let sums = "a948904f2f0f479b8f8197694b30184b0d2ed1c1cd2a1ec0fb85d299a192a44  hello.txt\n";
        let err = verify_file(&f, "hello.txt", sums).unwrap_err();
        match err {
            crate::errors::InstallError::ChecksumMismatch(msg) => {
                assert!(msg.contains("not listed"), "{msg}");
            }
            other => panic!("expected ChecksumMismatch(\"not listed\"), got {other:?}"),
        }
    }
}
