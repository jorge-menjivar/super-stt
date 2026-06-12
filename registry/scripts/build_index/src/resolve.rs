// SPDX-License-Identifier: GPL-3.0-only
//! Resolve the version + tag the indexer should publish for an entry.

use semver::Version;
use thiserror::Error;

use crate::github::{GitHub, Release};
use crate::registry_toml::Entry;

#[derive(Debug, Clone)]
pub struct Resolved {
    pub tag: String,
    pub version: Version,
    pub release: Release,
}

#[derive(Debug, Error)]
pub enum ResolveError {
    #[error("no releases on `{repo}`")]
    NoReleases { repo: String },
    #[error("no release tag matches prefix `{prefix}`")]
    NoMatchingPrefix { prefix: String },
    #[error("tag `{tag}` does not parse as semver after stripping prefix `{prefix:?}`")]
    BadSemver { tag: String, prefix: Option<String> },
    #[error(transparent)]
    Http(#[from] anyhow::Error),
}

pub async fn resolve(
    gh: &GitHub,
    owner_repo: &str,
    entry: &Entry,
) -> Result<Resolved, ResolveError> {
    let releases = if entry.tag_prefix.is_some() {
        gh.list_releases(owner_repo).await?
    } else {
        vec![gh.latest_release(owner_repo).await?]
    };
    select_release(releases, entry)
}

fn select_release(releases: Vec<Release>, entry: &Entry) -> Result<Resolved, ResolveError> {
    let max_cap = entry
        .max_version
        .as_deref()
        .map(parse_semver)
        .transpose()
        .map_err(|tag| ResolveError::BadSemver { tag, prefix: None })?;

    let mut best: Option<(Version, Release)> = None;
    for r in releases {
        if r.draft || r.prerelease {
            continue;
        }
        let stripped = match &entry.tag_prefix {
            Some(p) => match r.tag_name.strip_prefix(p.as_str()) {
                Some(rest) => rest,
                None => continue,
            },
            None => r.tag_name.strip_prefix('v').unwrap_or(&r.tag_name),
        };
        // The remainder must begin a semver (a leading digit). This enforces a
        // separator boundary so a bare `"v"` prefix or a prefix-of-prefix can't
        // claim a tag meant for another backend in the same repo.
        if !stripped.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        // unparseable tags are ignored, not errors
        let Ok(v) = Version::parse(stripped) else {
            continue;
        };
        if let Some(cap) = &max_cap
            && &v > cap
        {
            continue;
        }
        match &best {
            Some((cur, _)) if cur >= &v => {}
            _ => best = Some((v, r)),
        }
    }
    let (version, release) = best.ok_or_else(|| match &entry.tag_prefix {
        Some(p) => ResolveError::NoMatchingPrefix { prefix: p.clone() },
        None => ResolveError::NoReleases {
            repo: entry.repo.clone(),
        },
    })?;
    Ok(Resolved {
        tag: release.tag_name.clone(),
        version,
        release,
    })
}

fn parse_semver(s: &str) -> Result<Version, String> {
    Version::parse(s.strip_prefix('v').unwrap_or(s)).map_err(|_| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::Release;
    use crate::registry_toml::Entry;

    fn rel(tag: &str) -> Release {
        Release {
            tag_name: tag.into(),
            draft: false,
            prerelease: false,
            assets: vec![],
        }
    }

    fn entry(prefix: Option<&str>, max: Option<&str>) -> Entry {
        Entry {
            repo: "github.com/x/y".into(),
            subdir: None,
            tag_prefix: prefix.map(String::from),
            max_version: max.map(String::from),
            removed: false,
            removed_reason: None,
        }
    }

    #[test]
    fn picks_latest_semver_with_v_prefix() {
        let r = select_release(
            vec![rel("v1.0.0"), rel("v1.2.3"), rel("v1.1.0")],
            &entry(None, None),
        )
        .unwrap();
        assert_eq!(r.tag, "v1.2.3");
        assert_eq!(r.version, Version::new(1, 2, 3));
    }

    #[test]
    fn honors_max_version_cap() {
        let r = select_release(
            vec![rel("v1.0.0"), rel("v2.0.0")],
            &entry(None, Some("1.0.0")),
        )
        .unwrap();
        assert_eq!(r.version, Version::new(1, 0, 0));
    }

    #[test]
    fn filters_by_tag_prefix() {
        let r = select_release(
            vec![
                rel("openai-1.0.0"),
                rel("voxtral-2.0.0"),
                rel("openai-1.5.0"),
            ],
            &entry(Some("openai-"), None),
        )
        .unwrap();
        assert_eq!(r.version, Version::new(1, 5, 0));
    }

    #[test]
    fn skips_prerelease() {
        let mut releases = vec![rel("v1.0.0"), rel("v2.0.0")];
        releases[1].prerelease = true;
        let r = select_release(releases, &entry(None, None)).unwrap();
        assert_eq!(r.version, Version::new(1, 0, 0));
    }

    #[test]
    fn errors_when_no_match() {
        let err = select_release(vec![rel("v1.0.0")], &entry(Some("xyz-"), None)).unwrap_err();
        assert!(matches!(err, ResolveError::NoMatchingPrefix { .. }));
    }

    #[test]
    fn ignores_unparseable_tags() {
        let r = select_release(vec![rel("not-semver"), rel("v0.1.0")], &entry(None, None)).unwrap();
        assert_eq!(r.version, Version::new(0, 1, 0));
    }

    #[test]
    fn prefix_requires_digit_boundary() {
        // Prefix `a` must not match `a-1.0.0` (a separator, not a digit, follows).
        let err = select_release(vec![rel("a-1.0.0")], &entry(Some("a"), None)).unwrap_err();
        assert!(matches!(err, ResolveError::NoMatchingPrefix { .. }));
        // But `a-` does match it.
        let r = select_release(vec![rel("a-1.0.0")], &entry(Some("a-"), None)).unwrap();
        assert_eq!(r.version, Version::new(1, 0, 0));
    }
}
