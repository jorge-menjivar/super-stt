// SPDX-License-Identifier: GPL-3.0-only
//! Release resolution: pick the release to install (pin, stable, or beta) and
//! locate its tarball + `SHA256SUMS` assets for this host's target triple.

use super_stt_forge::{ForgeClient, Release, ReleaseKind, RepoRef};
use super_stt_registry_types::version::parse_version;

use crate::errors::InstallError;

/// Canonical `<host>/<owner>/<repo>` reference for Super STT itself, used for
/// self-update release lookups.
pub const REPO: &str = "github.com/jorge-menjivar/super-stt";

/// Map the running host's CPU architecture to a Rust target triple. Only the
/// architectures Super STT ships prebuilt Linux binaries for are supported.
///
/// # Errors
/// [`InstallError::UnsupportedArch`] on anything else (e.g. `mips`).
pub fn target_triple() -> Result<&'static str, InstallError> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("x86_64-unknown-linux-gnu"),
        "aarch64" => Ok("aarch64-unknown-linux-gnu"),
        other => Err(InstallError::UnsupportedArch(other.to_string())),
    }
}

/// The release tarball's asset name for `triple` at `tag`: the `-beta` suffix
/// is present exactly when `tag` parses as semver with a non-empty
/// prerelease component (e.g. `v0.2.3-beta.1`).
#[must_use]
pub fn tarball_name(triple: &str, tag: &str) -> String {
    let is_beta = parse_version(tag).is_some_and(|v| !v.pre.is_empty());
    if is_beta {
        format!("super-stt-{triple}-beta.tar.gz")
    } else {
        format!("super-stt-{triple}.tar.gz")
    }
}

/// A release resolved to a concrete, installable tarball + checksums asset
/// pair for the running host.
pub struct ResolvedTarget {
    pub release: Release,
    pub tarball_url: String,
    pub tarball_name: String,
    pub sums_url: String,
}

/// Select a release from `releases` (newest-first, as forges return them):
/// `pin`, if given, must match a tag exactly; otherwise the highest-versioned
/// release of the requested channel wins (`beta` allows [`ReleaseKind::Prerelease`]
/// in addition to [`ReleaseKind::Published`]; [`ReleaseKind::Draft`] is never
/// selectable).
fn pick_release<'a>(
    releases: &'a [Release],
    pin: Option<&str>,
    beta: bool,
) -> Result<&'a Release, InstallError> {
    if let Some(tag) = pin {
        return releases.iter().find(|r| r.tag == tag).ok_or_else(|| {
            InstallError::NoReleaseFound(format!("tag {tag} not in the latest 100 releases"))
        });
    }
    releases
        .iter()
        .filter(|r| match r.kind {
            ReleaseKind::Published => true,
            ReleaseKind::Prerelease => beta,
            ReleaseKind::Draft => false,
        })
        .filter_map(|r| parse_version(&r.tag).map(|v| (v, r)))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, r)| r)
        .ok_or_else(|| {
            InstallError::NoReleaseFound(format!(
                "no {} release found",
                if beta { "beta" } else { "stable" }
            ))
        })
}

/// Find `name` among `release`'s assets and return its download URL.
///
/// # Errors
/// [`InstallError::DownloadFailed`] naming the missing asset when `release`
/// does not carry one called `name`.
fn required_asset<'a>(release: &'a Release, name: &str) -> Result<&'a str, InstallError> {
    release
        .assets
        .iter()
        .find(|a| a.name == name)
        .map(|a| a.download_url.as_str())
        .ok_or_else(|| {
            InstallError::DownloadFailed(format!("release {} is missing asset {name}", release.tag))
        })
}

/// Resolve the release + this host's tarball/checksums assets to install.
///
/// Fetches up to one page (100) of releases from `client` — a single call
/// covers both the pinned and the unpinned/latest cases; a pin older than the
/// first 100 releases is out of scope and reported as [`InstallError::NoReleaseFound`].
///
/// # Errors
/// [`InstallError::NoReleaseFound`] when the release list was fetched but no
/// release matches `pin`/`beta`; [`InstallError::DownloadFailed`] when
/// fetching the release list itself fails, or when the selected release is
/// missing the tarball or `SHA256SUMS` asset for `triple`.
pub async fn resolve_target(
    client: &dyn ForgeClient,
    repo: &RepoRef,
    pin: Option<&str>,
    beta: bool,
    triple: &str,
) -> Result<ResolvedTarget, InstallError> {
    // A transport/API failure to even fetch the release list is a download
    // problem, not "nothing matched" — `NoReleaseFound` is reserved for the
    // list coming back and no release satisfying `pin`/`beta`.
    let releases = client
        .list_releases(repo)
        .await
        .map_err(|e| InstallError::DownloadFailed(e.to_string()))?;
    let release = pick_release(&releases, pin, beta)?;
    let name = tarball_name(triple, &release.tag);
    let tarball_url = required_asset(release, &name)?.to_string();
    let sums_url = required_asset(release, "SHA256SUMS")?.to_string();
    Ok(ResolvedTarget {
        release: release.clone(),
        tarball_url,
        tarball_name: name,
        sums_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use super_stt_forge::{Release, ReleaseKind};

    fn rel(tag: &str, kind: ReleaseKind) -> Release {
        Release {
            tag: tag.to_string(),
            kind,
            assets: Vec::new(),
        }
    }

    #[test]
    fn pin_finds_exact_tag_or_errors() {
        let rels = vec![
            rel("v0.2.2", ReleaseKind::Published),
            rel("v0.2.1", ReleaseKind::Published),
        ];
        assert_eq!(
            pick_release(&rels, Some("v0.2.1"), false).unwrap().tag,
            "v0.2.1"
        );
        assert!(matches!(
            pick_release(&rels, Some("v9.9.9"), false),
            Err(crate::errors::InstallError::NoReleaseFound(_))
        ));
    }

    #[test]
    fn unpinned_takes_highest_of_channel() {
        let rels = vec![
            rel("v0.2.3-beta.1", ReleaseKind::Prerelease),
            rel("v0.2.2", ReleaseKind::Published),
            rel("v9.9.9", ReleaseKind::Draft),
        ];
        assert_eq!(pick_release(&rels, None, false).unwrap().tag, "v0.2.2");
        assert_eq!(
            pick_release(&rels, None, true).unwrap().tag,
            "v0.2.3-beta.1"
        );
    }

    #[test]
    fn tarball_name_follows_channel_of_the_tag() {
        assert_eq!(
            tarball_name("x86_64-unknown-linux-gnu", "v0.2.3-beta.1"),
            "super-stt-x86_64-unknown-linux-gnu-beta.tar.gz"
        );
        assert_eq!(
            tarball_name("x86_64-unknown-linux-gnu", "v0.2.2"),
            "super-stt-x86_64-unknown-linux-gnu.tar.gz"
        );
    }

    #[test]
    fn resolve_requires_tarball_and_sums_assets() {
        // pick_release result must carry both the tarball for this triple and
        // SHA256SUMS; missing either is an error naming the missing asset.
        // Exercise via the small helper `required_asset(&release, name)`.
        let r = rel("v0.2.2", ReleaseKind::Published); // no assets
        assert!(matches!(
            required_asset(&r, "SHA256SUMS"),
            Err(crate::errors::InstallError::DownloadFailed(_))
        ));
    }
}
