// SPDX-License-Identifier: GPL-3.0-only
//! License policy for registry publication.
//!
//! A backend's `[backend].license` must be either a recognized open-source
//! license or the explicit [`OTHER`] escape. "Recognized" means a current
//! (non-deprecated) SPDX license identifier that the SPDX list marks
//! OSI-approved or FSF Free/Libre. The set is sourced from the `spdx` crate's
//! embedded license list, so validation runs offline (no network) and the
//! generated JSON Schema can embed the full set of accepted values inline.
//!
//! [`is_acceptable`] is the gate the registry indexer enforces;
//! [`accepted_schema_values`] feeds the schema's `enum` from the *same*
//! predicate, so the editor and the indexer can never disagree.

/// The explicit escape hatch: a license outside the recognized open-source set
/// (a custom or uncommon license). Declaring it is a conscious choice, not an
/// omission — the indexer still publishes the backend, shown as "Other".
pub const OTHER: &str = "other";

/// Whether SPDX `flags` mark a license as acceptable for publication: a
/// non-deprecated license that is OSI-approved or FSF Free/Libre.
fn flags_are_foss(flags: spdx::flags::Type) -> bool {
    flags & spdx::flags::IS_DEPRECATED == 0
        && (flags & spdx::flags::IS_OSI_APPROVED != 0 || flags & spdx::flags::IS_FSF_LIBRE != 0)
}

/// Whether `license` is acceptable for registry publication: the literal
/// [`OTHER`], or a single recognized open-source SPDX identifier.
///
/// The match is exact and case-sensitive — SPDX identifiers are case-sensitive
/// (`Apache-2.0`, not `apache-2.0`), and a typo must fail rather than be
/// silently coerced. License *expressions* (`MIT OR Apache-2.0`) are not
/// accepted; declare a single identifier or [`OTHER`].
#[must_use]
pub fn is_acceptable(license: &str) -> bool {
    license == OTHER || spdx::license_id(license).is_some_and(|id| flags_are_foss(id.flags))
}

/// Every recognized open-source SPDX identifier plus [`OTHER`], for embedding
/// as the `enum` of accepted values in the generated schema. Built from the
/// same predicate as [`is_acceptable`], so the schema and the indexer never
/// disagree. Ordered by the `spdx` crate's (alphabetical) identifier list,
/// with [`OTHER`] last.
#[must_use]
pub fn accepted_schema_values() -> Vec<&'static str> {
    let mut values: Vec<&'static str> = spdx::identifiers::LICENSES
        .iter()
        .filter(|l| flags_are_foss(l.flags))
        .map(|l| l.name)
        .collect();
    values.push(OTHER);
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_common_foss_identifiers() {
        for id in ["Apache-2.0", "MIT", "GPL-3.0-only", "MPL-2.0", "BSD-3-Clause"] {
            assert!(is_acceptable(id), "{id} should be acceptable");
        }
    }

    #[test]
    fn accepts_the_other_escape() {
        assert!(is_acceptable(OTHER));
    }

    #[test]
    fn rejects_typos_and_non_identifiers() {
        // Not SPDX ids: a typo, a casing slip, a human label, and the empty
        // string must all fail rather than be coerced.
        for bad in ["Apache2", "apache-2.0", "MIT License", "GPLv3", ""] {
            assert!(!is_acceptable(bad), "{bad:?} must not be acceptable");
        }
    }

    #[test]
    fn rejects_expressions() {
        // A single identifier is required; compound expressions are not parsed.
        assert!(!is_acceptable("MIT OR Apache-2.0"));
        assert!(!is_acceptable("Apache-2.0 WITH LLVM-exception"));
    }

    #[test]
    fn rejects_valid_spdx_that_is_not_foss() {
        // Real SPDX identifiers that are neither OSI-approved nor FSF-libre:
        // a non-commercial Creative Commons license and a deprecated id.
        assert!(!is_acceptable("CC-BY-NC-4.0"));
        assert!(!is_acceptable("GPL-3.0"), "the bare deprecated form must fail");
    }

    #[test]
    fn schema_values_match_the_predicate_and_include_other() {
        let values = accepted_schema_values();
        assert_eq!(*values.last().unwrap(), OTHER);
        // Every non-`other` value must itself be acceptable, and a few known
        // ids must be present.
        for v in &values {
            assert!(is_acceptable(v), "{v} listed but not acceptable");
        }
        for id in ["Apache-2.0", "MIT", "GPL-3.0-only"] {
            assert!(values.contains(&id), "{id} missing from schema enum");
        }
        // Sanity: the FOSS subset is a few hundred ids, not the whole SPDX list
        // and not a near-empty list.
        assert!(values.len() > 100, "suspiciously few values: {}", values.len());
    }
}
