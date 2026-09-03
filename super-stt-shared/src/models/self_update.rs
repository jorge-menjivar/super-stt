// SPDX-License-Identifier: GPL-3.0-only
//! Wire shape of `GET /v1/update` and `POST /v1/update/check`.
//! Contract: docs/protocol/endpoints/v1/update.md

use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfUpdateStatus {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub checked_at: Option<String>,
    pub last_check_error: Option<String>,
    pub beta_optin_effective: bool,
    pub installer_asset: Option<InstallerAsset>,
}

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallerAsset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// Hex SHA-256 of the binary at `url`, from the release's `SHA256SUMS`
    /// asset. Always present when `installer_asset` is non-null — clients
    /// MUST verify the downloaded bytes against this before executing it
    /// (the daemon omits `installer_asset` entirely rather than publish one
    /// without a verifiable digest).
    pub sha256: String,
}

#[cfg(test)]
mod tests {
    use super::SelfUpdateStatus;

    // Pinned to the documented example so daemon and app can't drift apart.
    #[test]
    fn deserializes_documented_shape() {
        let json = r#"{
            "current_version": "0.2.2-beta.2",
            "latest_version": "v0.2.3-beta.1",
            "update_available": true,
            "checked_at": "2026-08-20T17:00:00Z",
            "last_check_error": null,
            "beta_optin_effective": true,
            "installer_asset": {
                "name": "super-stt-install-x86_64-unknown-linux-gnu",
                "url": "https://github.com/jorge-menjivar/super-stt/releases/download/v0.2.3-beta.1/super-stt-install-x86_64-unknown-linux-gnu",
                "size": 8388608,
                "sha256": "a3f2c8b1d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1"
            }
        }"#;
        let s: SelfUpdateStatus = serde_json::from_str(json).unwrap();
        assert!(s.update_available);
        let asset = s.installer_asset.unwrap();
        assert_eq!(asset.size, 8_388_608);
        assert_eq!(
            asset.sha256,
            "a3f2c8b1d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1"
        );
    }
}
