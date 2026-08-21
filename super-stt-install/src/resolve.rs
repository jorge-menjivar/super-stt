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
#[derive(Debug)]
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

    fn rel_with_assets(tag: &str, kind: ReleaseKind, assets: &[&str]) -> Release {
        Release {
            tag: tag.to_string(),
            kind,
            assets: assets
                .iter()
                .map(|name| super_stt_forge::ReleaseAsset {
                    name: (*name).to_string(),
                    download_url: format!("https://example.invalid/{name}"),
                    size: 0,
                })
                .collect(),
        }
    }

    #[test]
    fn pin_absent_errors_no_release_found() {
        let rels = vec![rel("v0.2.2", ReleaseKind::Published)];
        assert!(matches!(
            pick_release(&rels, Some("v9.9.9"), false),
            Err(crate::errors::InstallError::NoReleaseFound(_))
        ));
    }

    #[test]
    fn unpinned_stable_never_selects_a_prerelease_or_draft() {
        let rels = vec![
            rel("v9.9.9-beta.1", ReleaseKind::Prerelease),
            rel("v9.9.8", ReleaseKind::Draft), // higher than any published tag
            rel("v0.2.2", ReleaseKind::Published),
            rel("v0.2.1", ReleaseKind::Published),
        ];
        // Highest PUBLISHED release wins — not the higher-versioned
        // prerelease or draft.
        assert_eq!(pick_release(&rels, None, false).unwrap().tag, "v0.2.2");
    }

    #[test]
    fn unpinned_beta_includes_prereleases_but_never_drafts() {
        let rels = vec![
            rel("v9.9.9", ReleaseKind::Draft), // must never win, even in beta mode
            rel("v0.2.3-beta.2", ReleaseKind::Prerelease),
            rel("v0.2.2", ReleaseKind::Published),
        ];
        assert_eq!(
            pick_release(&rels, None, true).unwrap().tag,
            "v0.2.3-beta.2"
        );
    }

    #[test]
    fn unparsable_tags_are_ignored() {
        let rels = vec![
            rel("not-a-version", ReleaseKind::Published),
            rel("v0.2.2", ReleaseKind::Published),
        ];
        assert_eq!(pick_release(&rels, None, false).unwrap().tag, "v0.2.2");
        // All-unparsable list: no release satisfies the channel filter.
        let all_bad = vec![rel("garbage", ReleaseKind::Published)];
        assert!(matches!(
            pick_release(&all_bad, None, false),
            Err(crate::errors::InstallError::NoReleaseFound(_))
        ));
    }

    #[test]
    fn empty_release_list_errors() {
        let rels: Vec<Release> = Vec::new();
        assert!(matches!(
            pick_release(&rels, None, false),
            Err(crate::errors::InstallError::NoReleaseFound(_))
        ));
        assert!(matches!(
            pick_release(&rels, Some("v1.0.0"), false),
            Err(crate::errors::InstallError::NoReleaseFound(_))
        ));
    }

    #[test]
    fn beta_only_list_with_beta_false_errors() {
        let rels = vec![rel("v0.2.3-beta.1", ReleaseKind::Prerelease)];
        assert!(matches!(
            pick_release(&rels, None, false),
            Err(crate::errors::InstallError::NoReleaseFound(_))
        ));
        // The same list, with beta requested, does resolve.
        assert_eq!(
            pick_release(&rels, None, true).unwrap().tag,
            "v0.2.3-beta.1"
        );
    }

    #[tokio::test]
    async fn resolve_target_happy_path_returns_both_urls() {
        super_stt_forge::install_crypto_provider();
        let mut s = mockito::Server::new_async().await;
        let base = s.url();
        let releases_json = serde_json::json!([{
            "tag_name": "v0.2.3-beta.1",
            "draft": false,
            "prerelease": true,
            "assets": [
                {
                    "name": "super-stt-x86_64-unknown-linux-gnu-beta.tar.gz",
                    "browser_download_url": format!("{base}/tarball"),
                    "size": 10,
                },
                {
                    "name": "SHA256SUMS",
                    "browser_download_url": format!("{base}/sums"),
                    "size": 10,
                },
            ],
        }])
        .to_string();
        let _m = s
            .mock(
                "GET",
                "/repos/jorge-menjivar/super-stt/releases?per_page=100",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(&releases_json)
            .create_async()
            .await;

        let repo = super_stt_forge::RepoRef::parse(REPO).unwrap();
        let client = super_stt_forge::Github::new(base.clone(), None);
        let target = resolve_target(&client, &repo, None, true, "x86_64-unknown-linux-gnu")
            .await
            .unwrap();
        assert_eq!(target.release.tag, "v0.2.3-beta.1");
        assert_eq!(target.tarball_url, format!("{base}/tarball"));
        assert_eq!(target.sums_url, format!("{base}/sums"));
        assert_eq!(
            target.tarball_name,
            "super-stt-x86_64-unknown-linux-gnu-beta.tar.gz"
        );
    }

    #[tokio::test]
    async fn resolve_target_errors_when_release_is_missing_sha256sums() {
        super_stt_forge::install_crypto_provider();
        let mut s = mockito::Server::new_async().await;
        let base = s.url();
        let releases_json = serde_json::json!([{
            "tag_name": "v0.2.2",
            "draft": false,
            "prerelease": false,
            "assets": [
                {
                    "name": "super-stt-x86_64-unknown-linux-gnu.tar.gz",
                    "browser_download_url": format!("{base}/tarball"),
                    "size": 10,
                },
            ],
        }])
        .to_string();
        let _m = s
            .mock(
                "GET",
                "/repos/jorge-menjivar/super-stt/releases?per_page=100",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(&releases_json)
            .create_async()
            .await;

        let repo = super_stt_forge::RepoRef::parse(REPO).unwrap();
        let client = super_stt_forge::Github::new(base.clone(), None);
        let err = resolve_target(&client, &repo, None, false, "x86_64-unknown-linux-gnu")
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            crate::errors::InstallError::DownloadFailed(_)
        ));
    }

    #[tokio::test]
    async fn resolve_target_errors_when_release_is_missing_the_arch_tarball() {
        super_stt_forge::install_crypto_provider();
        let mut s = mockito::Server::new_async().await;
        let base = s.url();
        let releases_json = serde_json::json!([{
            "tag_name": "v0.2.2",
            "draft": false,
            "prerelease": false,
            "assets": [
                {
                    "name": "SHA256SUMS",
                    "browser_download_url": format!("{base}/sums"),
                    "size": 10,
                },
            ],
        }])
        .to_string();
        let _m = s
            .mock(
                "GET",
                "/repos/jorge-menjivar/super-stt/releases?per_page=100",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(&releases_json)
            .create_async()
            .await;

        let repo = super_stt_forge::RepoRef::parse(REPO).unwrap();
        let client = super_stt_forge::Github::new(base.clone(), None);
        let err = resolve_target(&client, &repo, None, false, "x86_64-unknown-linux-gnu")
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            crate::errors::InstallError::DownloadFailed(_)
        ));
    }

    #[test]
    fn required_asset_missing_names_the_asset() {
        let r = rel_with_assets("v0.2.2", ReleaseKind::Published, &["SHA256SUMS"]);
        let err = required_asset(&r, "super-stt-x86_64-unknown-linux-gnu.tar.gz").unwrap_err();
        match err {
            crate::errors::InstallError::DownloadFailed(msg) => {
                assert!(
                    msg.contains("super-stt-x86_64-unknown-linux-gnu.tar.gz"),
                    "{msg}"
                );
            }
            other => panic!("expected DownloadFailed, got {other:?}"),
        }
    }

    #[test]
    fn target_triple_resolves_the_real_hosts_arch() {
        // `target_triple` switches on `std::env::consts::ARCH`, fixed at
        // compile time for this test binary — the unsupported-arch branch
        // (anything but x86_64/aarch64) isn't reachable here without cfg
        // tricks (building a whole separate test binary for e.g. `mips`),
        // so it's not exercised as a unit test; `errors.rs`'s
        // `error_codes_are_the_documented_closed_set` test already covers
        // `UnsupportedArch`'s `code()` mapping directly. This just confirms
        // the happy path: the real host's arch resolves to a triple.
        assert!(target_triple().is_ok());
    }
}
