// SPDX-License-Identifier: GPL-3.0-only
use thiserror::Error;

/// Closed error set; `code()` values are the wire contract with the app
/// (docs/superpowers/specs/2026-08-20-self-update-design.md §1).
#[derive(Debug, Error)]
pub enum InstallError {
    #[error("unsupported architecture: {0}")]
    UnsupportedArch(String),
    #[error("no matching release found: {0}")]
    NoReleaseFound(String),
    #[error("download failed: {0}")]
    DownloadFailed(String),
    #[error("checksum mismatch: {0}")]
    ChecksumMismatch(String),
    #[error("cannot escalate privileges: {0}")]
    EscalationUnavailable(String),
    #[error("authorization was denied")]
    EscalationDenied,
    #[error("install failed: {0}")]
    InstallFailed(String),
    #[error("post-install failed: {0}")]
    PostInstallFailed(String),
}

impl InstallError {
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedArch(_) => "unsupported_arch",
            Self::NoReleaseFound(_) => "no_release_found",
            Self::DownloadFailed(_) => "download_failed",
            Self::ChecksumMismatch(_) => "checksum_mismatch",
            Self::EscalationUnavailable(_) => "escalation_unavailable",
            Self::EscalationDenied => "escalation_denied",
            Self::InstallFailed(_) => "install_failed",
            Self::PostInstallFailed(_) => "post_install_failed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::InstallError;

    #[test]
    fn error_codes_are_the_documented_closed_set() {
        use InstallError::*;
        let cases: Vec<(InstallError, &str)> = vec![
            (UnsupportedArch("mips".into()), "unsupported_arch"),
            (NoReleaseFound("v9.9.9".into()), "no_release_found"),
            (DownloadFailed("timeout".into()), "download_failed"),
            (
                ChecksumMismatch("super-stt-x.tar.gz".into()),
                "checksum_mismatch",
            ),
            (
                EscalationUnavailable("no pkexec".into()),
                "escalation_unavailable",
            ),
            (EscalationDenied, "escalation_denied"),
            (InstallFailed("copy failed".into()), "install_failed"),
            (
                PostInstallFailed("restart failed".into()),
                "post_install_failed",
            ),
        ];
        for (e, code) in cases {
            assert_eq!(e.code(), code);
        }
    }
}
