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
    #[error("entry `{key}` must declare an `id` (see registry/README.md)")]
    MissingId { key: String },
    #[error("entry `{key}`: `id = {id:?}` is not a valid reverse-DNS id")]
    InvalidId { key: String, id: String },
    #[error("entries `{a}` and `{b}` both declare `id = {id:?}`")]
    DuplicateId { a: String, b: String, id: String },
}

/// Entries that predate the `id` requirement. They parse without one; every
/// other entry must declare one. Remove a key from this list once its entry
/// declares an `id` — the list is meant to shrink to empty.
const GRANDFATHERED: &[&str] = &[
    "deepgram",
    "mistral",
    "openai",
    "qwen3_asr",
    "voxtral",
    "whisper",
];

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
        let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
        for (key, e) in &raw {
            let Some(id) = e.id.as_deref() else { continue };
            if let Some(prev) = seen.insert(id, key) {
                return Err(ParseError::DuplicateId {
                    a: prev.to_string(),
                    b: key.clone(),
                    id: id.to_string(),
                });
            }
        }
        validate_monorepo_groups(&raw)?;
        Ok(Self(raw))
    }
}

fn validate_entry(id_key: &str, e: &Entry) -> Result<(), ParseError> {
    if !id_key
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err(ParseError::Entry {
            id: id_key.into(),
            reason: "id must be ascii lowercase, digits, `-`, `_`".into(),
        });
    }
    if e.repo.is_empty() {
        return Err(ParseError::Entry {
            id: id_key.into(),
            reason: "`repo` is required".into(),
        });
    }
    if let Some(sd) = &e.subdir
        && !super_stt_registry_types::is_safe_relative_path(sd)
    {
        return Err(ParseError::Entry {
            id: id_key.into(),
            reason: format!("`subdir = {sd:?}` is not a safe relative path"),
        });
    }
    match &e.id {
        Some(id) if !super_stt_registry_types::backend_id::is_valid(id) => {
            return Err(ParseError::InvalidId {
                key: id_key.into(),
                id: id.clone(),
            });
        }
        None if !GRANDFATHERED.contains(&id_key) => {
            return Err(ParseError::MissingId { key: id_key.into() });
        }
        _ => {}
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
            "[good]\nid = \"com.example.good\"\nrepo = \"github.com/x/y\"\nforge = \"github\"\nsubdir = \"my..backend/v2\"\n",
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
            id = "com.example.a"
            repo = "github.com/x/mono"
            forge = "github"
            [b]
            id = "com.example.b"
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
            id = "com.example.a"
            repo = "github.com/x/mono"
            forge = "github"
            tag_prefix = "v"
            [b]
            id = "com.example.b"
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
            id = "com.example.a"
            repo = "github.com/x/mono"
            forge = "github"
            tag_prefix = "voxtral-"
            [b]
            id = "com.example.b"
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
            id = "com.example.a"
            repo = "github.com/x/mono"
            forge = "github"
            tag_prefix = "a-"
            [b]
            id = "com.example.b"
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
            id = "com.example.a"
            repo = "github.com/x/mono"
            forge = "github"
            [b]
            id = "com.example.b"
            repo = "github.com/x/mono"
            forge = "github"
            removed = true
        "#,
        )
        .unwrap();
        assert_eq!(r.0.len(), 2);
    }
}

#[cfg(test)]
mod id_tests {
    use super::{ParseError, Registry};

    const NEW_ENTRY: &str = r#"
[brand-new]
    id    = "com.example.brand-new"
    repo  = "github.com/example/super-stt-brand-new"
    forge = "github"
"#;

    #[test]
    fn accepts_a_new_entry_with_a_valid_unique_id() {
        let r = Registry::parse(NEW_ENTRY).expect("parses");
        assert_eq!(
            r.0["brand-new"].id.as_deref(),
            Some("com.example.brand-new")
        );
    }

    #[test]
    fn rejects_a_new_entry_without_an_id() {
        let text = "[brand-new]\n    repo = \"github.com/example/x\"\n    forge = \"github\"\n";
        let err = Registry::parse(text).unwrap_err();
        assert!(matches!(err, ParseError::MissingId { .. }), "{err:?}");
    }

    #[test]
    fn accepts_a_grandfathered_entry_without_an_id() {
        let text = "[voxtral]\n    repo = \"github.com/jorge-menjivar/super-stt-voxtral\"\n    forge = \"github\"\n";
        Registry::parse(text).expect("existing entries are exempt");
    }

    #[test]
    fn rejects_a_malformed_id() {
        let text = NEW_ENTRY.replace("com.example.brand-new", "brandnew");
        let err = Registry::parse(&text).unwrap_err();
        assert!(matches!(err, ParseError::InvalidId { .. }), "{err:?}");
    }

    #[test]
    fn rejects_two_entries_sharing_an_id() {
        let text = format!(
            "{NEW_ENTRY}\n[other]\n    id    = \"com.example.brand-new\"\n    repo  = \"github.com/example/super-stt-other\"\n    forge = \"github\"\n"
        );
        let err = Registry::parse(&text).unwrap_err();
        assert!(matches!(err, ParseError::DuplicateId { .. }), "{err:?}");
    }

    /// The shipped file must keep parsing as the requirement lands.
    #[test]
    fn the_in_repo_registry_parses() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap();
        let text = std::fs::read_to_string(root.join("registry/registry.toml")).unwrap();
        Registry::parse(&text).expect("registry.toml parses");
    }
}
