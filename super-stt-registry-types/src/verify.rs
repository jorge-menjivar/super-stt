// SPDX-License-Identifier: GPL-3.0-only
//! Shared download-verification policy: predicates the daemon (install) and the
//! indexer (publish) must apply identically so a backend that passes publishing
//! also installs. Mostly pure logic — most of this module has no I/O or
//! hashing backend, and callers compute a digest with whatever library they
//! already use and pass the hex string in. [`file_sha256_hex`] and
//! [`parse_sha256sums`] are the exception: streaming file hashing and
//! `SHA256SUMS`-listing parsing used identically by the installer (its own
//! tarball) and the daemon (the self-update installer binary), promoted here
//! so both stay in lock-step. [`random_hex_suffix`] is a second, smaller
//! exception: the unpredictable-name pattern the installer's own staging
//! directory and the app's self-update download directory both need.

use std::io::Read;
use std::path::Path;

/// Per-file ceiling when unpacking a subprocess tarball. A single bundled
/// library (e.g. a CUDA `.so`) can be large, so this is generous.
pub const MAX_TARBALL_ENTRY_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Floor for the total uncompressed-output ceiling. The actual cap scales with
/// the compressed archive (see [`unpack_cap`]) so a large but legitimate bundle
/// (a CUDA `PyTorch` tarball unpacks to several GiB) is allowed while a zip-bomb —
/// a tiny archive with huge output — is still rejected.
pub const MAX_TARBALL_TOTAL_FLOOR: u64 = 4 * 1024 * 1024 * 1024;
/// Maximum unpacked-output-to-compressed-archive ratio. gzip over already-packed
/// ML libraries stays far below this; a zip-bomb is far above it.
pub const MAX_DECOMP_RATIO: u64 = 5;

/// Ceiling for a `backend.toml` manifest asset — manifests are tiny; this only
/// bounds a hostile or mistaken upload. Enforced identically at publish (the
/// indexer), install-time download, and custom-repo resolve, so a manifest that
/// passes publishing also installs (mirrors the tarball budgets above).
pub const MAX_MANIFEST_BYTES: u64 = 256 * 1024;

/// The uncompressed-output ceiling for an archive of `archive_size` compressed
/// bytes: scales with the input but never drops below the floor.
#[must_use]
pub fn unpack_cap(archive_size: u64) -> u64 {
    archive_size
        .saturating_mul(MAX_DECOMP_RATIO)
        .max(MAX_TARBALL_TOTAL_FLOOR)
}

/// Reason a tar entry path is unsafe to unpack, or `None` when it is safe.
///
/// A safe entry has a relative, non-escaping path and is not a symlink. This is
/// the single predicate both the daemon (install-time extraction) and the
/// indexer (publish-time validation) apply, so a tarball that passes publishing
/// also passes installation.
#[must_use]
pub fn tar_entry_unsafe_reason(entry_path: &str, is_symlink: bool) -> Option<String> {
    if entry_path.starts_with('/') || entry_path.contains("..") {
        return Some(entry_path.to_string());
    }
    if is_symlink {
        return Some(format!("symlink: {entry_path}"));
    }
    None
}

/// Fold one tar entry of `entry_size` bytes into the running unpacked total,
/// enforcing the per-entry cap and the archive-scaled total cap. Returns the
/// new running total, or `Err(reason)` if either budget is breached — the same
/// policy the daemon enforces while unpacking and the indexer must enforce at
/// publish so a zip-bomb is caught before it ships.
///
/// # Errors
/// Returns a human-readable reason when `entry_size` exceeds
/// [`MAX_TARBALL_ENTRY_BYTES`] or `running_total + entry_size` exceeds
/// `total_cap` (typically [`unpack_cap`] of the compressed size).
pub fn tar_budget_step(entry_size: u64, running_total: u64, total_cap: u64) -> Result<u64, String> {
    if entry_size > MAX_TARBALL_ENTRY_BYTES {
        return Err(format!("entry exceeds {MAX_TARBALL_ENTRY_BYTES} bytes"));
    }
    let total = running_total.saturating_add(entry_size);
    if total > total_cap {
        return Err(format!("archive output exceeds {total_cap} bytes"));
    }
    Ok(total)
}

/// Whether a computed SHA-256 hex digest matches an expected one.
///
/// The comparison is **case-insensitive**: `expected` originates from an
/// external `index.json` / manifest and may be upper- or mixed-case, while the
/// locally computed `actual` is lowercase hex. A case-sensitive `==` would
/// spuriously reject a valid uppercase pin. Leading/trailing whitespace is not
/// trimmed — pins are expected to be bare hex.
///
/// Callers own the "empty `expected` ⇒ skip verification" policy (the
/// unverified-source / no-pin case); this helper only answers "do these two
/// digests match", and an empty `expected` never matches a real digest.
#[must_use]
pub fn sha256_matches(actual: &str, expected: &str) -> bool {
    !expected.is_empty() && actual.eq_ignore_ascii_case(expected)
}

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

/// `n_bytes` random bytes from the system RNG, hex-encoded — for building an
/// unpredictable directory/file name (e.g. a staging directory a same-UID
/// process must not be able to guess or pre-create/race). Shared by
/// `super-stt-install`'s own staging directory (`StagingGuard`) and
/// `super-stt-app`'s self-update download directory, so both draw from the
/// same RNG/encoding in lock-step.
///
/// # Panics
/// If the system RNG is unavailable — a fatal host condition with no sane
/// "unpredictable" fallback.
#[must_use]
pub fn random_hex_suffix(n_bytes: usize) -> String {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut buf = vec![0u8; n_bytes];
    SystemRandom::new()
        .fill(&mut buf)
        .expect("system RNG unavailable");
    hex::encode(buf)
}

/// Compute the SHA-256 digest of the file at `path`, streaming it in 1 MiB
/// reads so a multi-hundred-MB tarball (or installer binary) is never fully
/// buffered in memory.
///
/// # Errors
/// Any I/O error opening or reading `path`.
pub fn file_sha256_hex(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
    }
    Ok(hex::encode(ctx.finish().as_ref()))
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_TARBALL_ENTRY_BYTES, MAX_TARBALL_TOTAL_FLOOR, file_sha256_hex, parse_sha256sums,
        random_hex_suffix, sha256_matches, tar_budget_step, tar_entry_unsafe_reason, unpack_cap,
    };

    #[test]
    fn tar_escape_paths_are_unsafe() {
        assert!(tar_entry_unsafe_reason("bin/qwen3-asr", false).is_none());
        assert!(tar_entry_unsafe_reason("model/weights.bin", false).is_none());
        assert!(tar_entry_unsafe_reason("/etc/passwd", false).is_some());
        assert!(tar_entry_unsafe_reason("../escape", false).is_some());
        assert!(tar_entry_unsafe_reason("a/../../b", false).is_some());
        // A symlink (even to a safe-looking path) is rejected.
        assert!(tar_entry_unsafe_reason("bin/link", true).is_some());
    }

    #[test]
    fn unpack_cap_scales_but_never_below_floor() {
        // Small archive → the floor, not archive_size * ratio.
        assert_eq!(unpack_cap(1000), MAX_TARBALL_TOTAL_FLOOR);
        // Large archive → scales by the ratio.
        let big = MAX_TARBALL_TOTAL_FLOOR; // * 5 > floor
        assert_eq!(unpack_cap(big), big * super::MAX_DECOMP_RATIO);
    }

    #[test]
    fn tar_budget_rejects_oversized_entry_and_total() {
        let cap = unpack_cap(1000); // == floor
        // Normal accumulation returns the new running total.
        assert_eq!(tar_budget_step(100, 0, cap).unwrap(), 100);
        assert_eq!(tar_budget_step(50, 100, cap).unwrap(), 150);
        // A single entry over the per-entry cap is rejected.
        assert!(tar_budget_step(MAX_TARBALL_ENTRY_BYTES + 1, 0, cap).is_err());
        // Crossing the total cap is rejected (zip-bomb: tiny archive, huge output).
        assert!(tar_budget_step(cap, 1, cap).is_err());
    }

    #[test]
    fn matches_are_case_insensitive() {
        let lower = "abc123def456";
        assert!(sha256_matches(lower, lower));
        assert!(sha256_matches(lower, "ABC123DEF456"));
        assert!(sha256_matches("ABC123DEF456", lower));
    }

    #[test]
    fn mismatch_and_empty_do_not_match() {
        assert!(!sha256_matches("abc123", "def456"));
        // An empty expected pin is never a match — callers treat empty as
        // "skip verification" *before* calling this.
        assert!(!sha256_matches("abc123", ""));
        assert!(!sha256_matches("", ""));
    }

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
    fn file_sha256_hex_matches_known_vector() {
        // sha256("hello world\n") — a standard test vector.
        let dir =
            std::env::temp_dir().join(format!("sstt-registry-verify-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("hello.txt");
        std::fs::write(&f, "hello world\n").unwrap();
        assert_eq!(
            file_sha256_hex(&f).unwrap(),
            "a948904f2f0f479b8f8197694b30184b0d2ed1c1cd2a1ec0fb85d299a192a447"
        );
        assert!(file_sha256_hex(&dir.join("missing.txt")).is_err());
    }

    #[test]
    fn random_hex_suffix_is_hex_of_the_requested_length_and_unpredictable() {
        let a = random_hex_suffix(8);
        let b = random_hex_suffix(8);
        assert_eq!(a.len(), 16, "8 bytes hex-encode to 16 hex chars");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        // Astronomically unlikely to collide if the RNG is actually being
        // used — a regression to a fixed/zeroed buffer would make this fail.
        assert_ne!(a, b);
    }
}
