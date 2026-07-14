// SPDX-License-Identifier: GPL-3.0-only
//! Shared version parsing + update comparison for registry backends.
//!
//! One home so the daemon (`POST /registry/backends/update`), the app (the
//! "Update available" affordance), and the indexer (release selection +
//! manifest validation) agree on what parses as a version and what counts as
//! "newer" — previously four subtly different implementations, including a
//! daemon string-equality check that happily downgraded (Tier 1 #31).

use semver::Version;

/// Parse a backend version string as semver, tolerating a single leading `v`
/// (`v1.2.3` is treated as `1.2.3`). Returns `None` for anything `semver`
/// cannot parse.
#[must_use]
pub fn parse_version(s: &str) -> Option<Version> {
    Version::parse(s.strip_prefix('v').unwrap_or(s)).ok()
}

/// Whether `candidate` is a strictly newer version than `installed`. Both must
/// parse via [`parse_version`]; if either does not, returns `false` so a
/// non-semver or malformed version never prompts an update — and, crucially,
/// never a downgrade (an older or equal `candidate` is not "available").
#[must_use]
pub fn update_available(installed: &str, candidate: &str) -> bool {
    match (parse_version(installed), parse_version(candidate)) {
        (Some(have), Some(want)) => want > have,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_version, update_available};

    #[test]
    fn parses_with_and_without_v_prefix() {
        assert_eq!(parse_version("1.2.3"), parse_version("v1.2.3"));
        assert!(parse_version("1.2.3").is_some());
        assert!(parse_version("nightly").is_none());
        assert!(parse_version("1.2").is_none()); // not full MAJOR.MINOR.PATCH
        assert!(parse_version("").is_none());
    }

    #[test]
    fn update_available_is_strictly_newer() {
        assert!(update_available("0.1.0", "0.2.0"));
        assert!(update_available("0.2.0", "0.10.0")); // numeric, not lexical
        assert!(update_available("1.0.0", "v1.0.1")); // v-prefix tolerated
    }

    #[test]
    fn no_update_for_equal_older_or_unparseable() {
        assert!(!update_available("1.2.3", "1.2.3")); // equal
        assert!(!update_available("2.0.0", "1.9.9")); // downgrade never offered
        assert!(!update_available("1.2", "1.3.0")); // partial installed version
        assert!(!update_available("1.0.0", "nightly")); // non-semver candidate
        assert!(!update_available("", "1.0.0")); // empty installed
    }
}
