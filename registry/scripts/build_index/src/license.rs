// SPDX-License-Identifier: GPL-3.0-only
//! License policy gate for registry publication. The accepted set — recognized
//! open-source SPDX identifiers (OSI-approved or FSF Free/Libre) plus the
//! explicit `other` escape — is defined canonically in
//! `super_stt_registry_types::license`; this maps its verdict onto the
//! indexer's `ManifestError` so a missing field and an unrecognized value stay
//! distinct in the build log.

use crate::manifest::ManifestError;

pub fn check(license: Option<&str>) -> Result<(), ManifestError> {
    let lic = license.ok_or(ManifestError::MissingLicense)?;
    if super_stt_registry_types::license::is_acceptable(lic) {
        Ok(())
    } else {
        Err(ManifestError::LicenseNotAllowed(lic.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_recognized_foss_licenses() {
        for id in ["Apache-2.0", "MIT", "GPL-3.0-only"] {
            check(Some(id)).unwrap_or_else(|e| panic!("{id} should pass: {e}"));
        }
    }

    #[test]
    fn allows_the_other_escape() {
        check(Some("other")).unwrap();
    }

    #[test]
    fn rejects_missing() {
        let err = check(None).unwrap_err();
        assert!(matches!(err, ManifestError::MissingLicense));
    }

    #[test]
    fn rejects_unrecognized_value() {
        // A non-SPDX label, an expression, and a valid-but-non-FOSS id all map
        // to the same "not allowed" verdict.
        for bad in ["Proprietary", "MIT OR Apache-2.0", "CC-BY-NC-4.0"] {
            let err = check(Some(bad)).unwrap_err();
            assert!(
                matches!(err, ManifestError::LicenseNotAllowed(_)),
                "{bad:?} should be rejected as not allowed, got {err:?}"
            );
        }
    }
}
