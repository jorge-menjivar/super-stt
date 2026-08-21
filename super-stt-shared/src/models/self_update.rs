// SPDX-License-Identifier: GPL-3.0-only
//! Wire shape of `GET /v1/update` and `POST /v1/update/check`.
//! Contract: docs/protocol/endpoints/v1/update.md

use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallerAsset {
    pub name: String,
    pub url: String,
    pub size: u64,
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
                "size": 8388608
            }
        }"#;
        let s: SelfUpdateStatus = serde_json::from_str(json).unwrap();
        assert!(s.update_available);
        assert_eq!(s.installer_asset.unwrap().size, 8_388_608);
    }
}
