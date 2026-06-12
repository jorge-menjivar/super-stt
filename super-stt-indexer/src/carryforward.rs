// SPDX-License-Identifier: GPL-3.0-only
//! Last-known-good carry-forward: when a new index build fails for an entry,
//! copy the prior `index.json` entry forward with an added `index_stale` field.

use crate::index_json::{IndexBackend, IndexStale};

/// How long a last-known-good entry may be carried forward after it first went
/// stale before the indexer drops it. Bounds how long a yanked or long-broken
/// release can pin an old (possibly vulnerable) version.
pub const MAX_STALENESS_DAYS: i64 = 30;

pub fn maybe_carry_forward(
    id: &str,
    prior: Option<&IndexBackend>,
    error: &str,
    attempted_version: &str,
    attempted_tag: &str,
    now_iso: &str,
    max_staleness_days: i64,
) -> Option<IndexBackend> {
    let prior = prior?;
    if prior.id != id {
        return None;
    }

    // Measure staleness from when the entry *first* went stale, not from this
    // build — so the window doesn't reset on every consecutive failed build.
    let since = prior
        .index_stale
        .as_ref()
        .map_or(now_iso, |s| s.since.as_str());

    // Once stale longer than the window, drop the entry instead of carrying it
    // forever. (If either timestamp is unparseable, fail safe and keep
    // carrying — a parse bug shouldn't yank a backend.)
    if let (Ok(since_dt), Ok(now_dt)) = (
        chrono::DateTime::parse_from_rfc3339(since),
        chrono::DateTime::parse_from_rfc3339(now_iso),
    ) && (now_dt - since_dt).num_days() > max_staleness_days
    {
        return None;
    }

    let mut copy = prior.clone();
    copy.index_stale = Some(IndexStale {
        latest_attempted: attempted_version.into(),
        tag: attempted_tag.into(),
        error: error.into(),
        since: since.into(),
    });
    Some(copy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index_json::*;

    fn dummy(id: &str) -> IndexBackend {
        IndexBackend {
            id: id.into(),
            source: "x".into(),
            version: "1.0.0".into(),
            tag: "v1.0.0".into(),
            name: id.into(),
            description: None,
            license: "Apache-2.0".into(),
            kind: "wasm".into(),
            contract: "v1".into(),
            entrypoint: format!("{id}.wasm"),
            allowed_hosts: vec![],
            online: false,
            supports_gpu: false,
            supports_cpu: false,
            models: vec![],
            secrets: vec![],
            options: vec![],
            assets: IndexAssets::default(),
            index_stale: None,
        }
    }

    #[test]
    fn carries_forward_with_index_stale_marker() {
        let prior = dummy("openai");
        let carried = maybe_carry_forward(
            "openai",
            Some(&prior),
            "asset missing",
            "1.5.0",
            "v1.5.0",
            "2026-05-29T18:00:00Z",
            30,
        )
        .unwrap();
        assert_eq!(carried.version, "1.0.0");
        let stale = carried.index_stale.unwrap();
        assert_eq!(stale.latest_attempted, "1.5.0");
        assert_eq!(stale.error, "asset missing");
        // First time stale: `since` is set to now.
        assert_eq!(stale.since, "2026-05-29T18:00:00Z");
    }

    #[test]
    fn preserves_original_since_and_drops_after_window() {
        // Already stale since an old date.
        let mut prior = dummy("openai");
        prior.index_stale = Some(IndexStale {
            latest_attempted: "1.4.0".into(),
            tag: "v1.4.0".into(),
            error: "broke".into(),
            since: "2026-01-01T00:00:00Z".into(),
        });
        // Within window: carried, and the original `since` is preserved.
        let carried = maybe_carry_forward(
            "openai",
            Some(&prior),
            "still broke",
            "1.5.0",
            "v1.5.0",
            "2026-01-20T00:00:00Z",
            30,
        )
        .unwrap();
        assert_eq!(carried.index_stale.unwrap().since, "2026-01-01T00:00:00Z");
        // Past the window: dropped.
        assert!(
            maybe_carry_forward(
                "openai",
                Some(&prior),
                "still broke",
                "1.5.0",
                "v1.5.0",
                "2026-06-01T00:00:00Z",
                30
            )
            .is_none()
        );
    }

    #[test]
    fn returns_none_when_no_prior() {
        assert!(maybe_carry_forward("openai", None, "x", "1.0.0", "v1.0.0", "now", 30).is_none());
    }
}
