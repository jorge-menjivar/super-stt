// SPDX-License-Identifier: GPL-3.0-only
//! License allowlist.

use crate::manifest::ManifestError;

const ALLOWED: &[&str] = &[
    "Apache-2.0", "MIT", "BSD-2-Clause", "BSD-3-Clause", "MPL-2.0",
    "GPL-3.0-only", "GPL-3.0-or-later", "ISC",
];

pub fn check(license: Option<&str>) -> Result<(), ManifestError> {
    let lic = license.ok_or(ManifestError::MissingLicense)?;
    if !ALLOWED.contains(&lic) {
        return Err(ManifestError::LicenseNotAllowed(lic.into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_apache() { check(Some("Apache-2.0")).unwrap(); }

    #[test]
    fn rejects_unknown() {
        let err = check(Some("Proprietary")).unwrap_err();
        assert!(matches!(err, ManifestError::LicenseNotAllowed(_)));
    }

    #[test]
    fn rejects_missing() {
        let err = check(None).unwrap_err();
        assert!(matches!(err, ManifestError::MissingLicense));
    }
}
