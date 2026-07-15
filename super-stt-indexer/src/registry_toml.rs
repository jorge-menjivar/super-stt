// SPDX-License-Identifier: GPL-3.0-only
//! Parse and structurally validate `registry/registry.toml`.

use std::collections::BTreeMap;

use thiserror::Error;

pub use super_stt_registry_types::entry::Entry;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("entry `{id}`: {reason}")]
    Entry { id: String, reason: String },
    #[error("entries `{a}` and `{b}` share repo `{repo}` but at least one has no `tag_prefix`")]
    MonorepoMissingPrefix { a: String, b: String, repo: String },
    #[error("entries `{a}` and `{b}` share repo `{repo}` and the same `tag_prefix = {prefix:?}`")]
    PrefixCollision {
        a: String,
        b: String,
        repo: String,
        prefix: String,
    },
    #[error(
        "entries `{a}` and `{b}` share repo `{repo}` and one `tag_prefix` is a prefix of the other"
    )]
    PrefixOverlap { a: String, b: String, repo: String },
}

/// Parsed registry file: id → entry. Backed by a `BTreeMap`, so iteration is in
/// sorted id order — not the file's declaration order.
#[derive(Debug, Clone)]
pub struct Registry(pub BTreeMap<String, Entry>);

impl Registry {
    pub fn parse(input: &str) -> Result<Self, ParseError> {
        let raw: BTreeMap<String, Entry> = toml::from_str(input)?;
        for (id, e) in &raw {
            validate_entry(id, e)?;
        }
        validate_monorepo_groups(&raw)?;
        Ok(Self(raw))
    }
}

fn validate_entry(id: &str, e: &Entry) -> Result<(), ParseError> {
    if !id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err(ParseError::Entry {
            id: id.into(),
            reason: "id must be ascii lowercase, digits, `-`, `_`".into(),
        });
    }
    if e.repo.is_empty() {
        return Err(ParseError::Entry {
            id: id.into(),
            reason: "`repo` is required".into(),
        });
    }
    if let Some(sd) = &e.subdir
        && !super_stt_registry_types::is_safe_relative_path(sd)
    {
        return Err(ParseError::Entry {
            id: id.into(),
            reason: format!("`subdir = {sd:?}` is not a safe relative path"),
        });
    }
    Ok(())
}

fn validate_monorepo_groups(raw: &BTreeMap<String, Entry>) -> Result<(), ParseError> {
    let mut by_repo: BTreeMap<&str, Vec<(&str, Option<&str>)>> = BTreeMap::new();
    for (id, e) in raw {
        if e.removed {
            continue;
        }
        by_repo
            .entry(e.repo.as_str())
            .or_default()
            .push((id, e.tag_prefix.as_deref()));
    }
    for (repo, members) in &by_repo {
        if members.len() < 2 {
            continue;
        }
        for (i, (a, prefix_a)) in members.iter().enumerate() {
            if prefix_a.is_none() {
                let (b, _) = members.iter().find(|(b, _)| b != a).unwrap();
                return Err(ParseError::MonorepoMissingPrefix {
                    a: (*a).into(),
                    b: (*b).into(),
                    repo: (*repo).into(),
                });
            }
            for (b, prefix_b) in &members[i + 1..] {
                if prefix_a == prefix_b {
                    return Err(ParseError::PrefixCollision {
                        a: (*a).into(),
                        b: (*b).into(),
                        repo: (*repo).into(),
                        prefix: prefix_a.unwrap_or_default().into(),
                    });
                }
                // A prefix that is a prefix of another (e.g. `voxtral-` and
                // `voxtral-mini-`) lets one entry's tag resolve as another's.
                if let (Some(pa), Some(pb)) = (*prefix_a, *prefix_b)
                    && (pa.starts_with(pb) || pb.starts_with(pa))
                {
                    return Err(ParseError::PrefixOverlap {
                        a: (*a).into(),
                        b: (*b).into(),
                        repo: (*repo).into(),
                    });
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_entry() {
        let r = Registry::parse(
            r#"
            [openai]
            repo = "github.com/jorge-menjivar/super-stt"
            forge = "github"
            subdir = "backends/openai"
            tag_prefix = "openai-"
        "#,
        )
        .unwrap();
        let e = r.0.get("openai").unwrap();
        assert_eq!(e.repo, "github.com/jorge-menjivar/super-stt");
        assert_eq!(e.subdir.as_deref(), Some("backends/openai"));
        assert_eq!(e.tag_prefix.as_deref(), Some("openai-"));
    }

    #[test]
    fn rejects_path_traversal_in_subdir() {
        let err = Registry::parse(
            r#"
            [bad]
            repo = "github.com/x/y"
            forge = "github"
            subdir = "../escape"
        "#,
        )
        .unwrap_err();
        assert!(matches!(err, ParseError::Entry { .. }));
    }

    #[test]
    fn subdir_uses_the_canonical_path_guard() {
        // The shared `is_safe_relative_path` is stricter than the old
        // substring check: it rejects empty components, `.`, and trailing
        // slashes, and accepts a benign `..`-containing filename.
        for bad in ["a//b", "./x", "models/", "a/../b"] {
            let err = Registry::parse(&format!(
                "[bad]\nrepo = \"github.com/x/y\"\nforge = \"github\"\nsubdir = \"{bad}\"\n"
            ))
            .unwrap_err();
            assert!(
                matches!(err, ParseError::Entry { .. }),
                "{bad:?} should be rejected"
            );
        }
        let ok = Registry::parse(
            "[good]\nrepo = \"github.com/x/y\"\nforge = \"github\"\nsubdir = \"my..backend/v2\"\n",
        )
        .unwrap();
        assert_eq!(
            ok.0.get("good").unwrap().subdir.as_deref(),
            Some("my..backend/v2")
        );
    }

    #[test]
    fn rejects_monorepo_without_tag_prefix() {
        let err = Registry::parse(
            r#"
            [a]
            repo = "github.com/x/mono"
            forge = "github"
            [b]
            repo = "github.com/x/mono"
            forge = "github"
        "#,
        )
        .unwrap_err();
        assert!(matches!(err, ParseError::MonorepoMissingPrefix { .. }));
    }

    #[test]
    fn rejects_tag_prefix_collision() {
        let err = Registry::parse(
            r#"
            [a]
            repo = "github.com/x/mono"
            forge = "github"
            tag_prefix = "v"
            [b]
            repo = "github.com/x/mono"
            forge = "github"
            tag_prefix = "v"
        "#,
        )
        .unwrap_err();
        assert!(matches!(err, ParseError::PrefixCollision { .. }));
    }

    #[test]
    fn rejects_prefix_that_is_a_prefix_of_another() {
        let err = Registry::parse(
            r#"
            [a]
            repo = "github.com/x/mono"
            forge = "github"
            tag_prefix = "voxtral-"
            [b]
            repo = "github.com/x/mono"
            forge = "github"
            tag_prefix = "voxtral-mini-"
        "#,
        )
        .unwrap_err();
        assert!(matches!(err, ParseError::PrefixOverlap { .. }));
    }

    #[test]
    fn allows_two_distinct_prefixes_on_same_repo() {
        let r = Registry::parse(
            r#"
            [a]
            repo = "github.com/x/mono"
            forge = "github"
            tag_prefix = "a-"
            [b]
            repo = "github.com/x/mono"
            forge = "github"
            tag_prefix = "b-"
        "#,
        )
        .unwrap();
        assert_eq!(r.0.len(), 2);
    }

    #[test]
    fn removed_entries_dont_count_for_collision() {
        // Two entries on the same repo, one removed → no collision.
        let r = Registry::parse(
            r#"
            [a]
            repo = "github.com/x/mono"
            forge = "github"
            [b]
            repo = "github.com/x/mono"
            forge = "github"
            removed = true
        "#,
        )
        .unwrap();
        assert_eq!(r.0.len(), 2);
    }
}
