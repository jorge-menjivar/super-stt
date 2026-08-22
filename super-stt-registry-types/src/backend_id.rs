// SPDX-License-Identifier: GPL-3.0-only
//! Format rules for `[backend].id` — the reverse-DNS identifier that names a
//! backend's install directory.

/// Longest permitted id. A backend id becomes a filesystem path component,
/// and 255 bytes is the component limit on the filesystems this runs on.
const MAX_LEN: usize = 255;

/// Whether `s` is a well-formed reverse-DNS backend id.
///
/// At least three `.`-separated segments, each beginning with a lowercase
/// ASCII letter and containing only lowercase letters, digits, and `-`, and
/// none ending in `-`. Rejects empty, leading, trailing, and consecutive dot
/// segments implicitly: an empty segment has no leading letter.
#[must_use]
pub fn is_valid(s: &str) -> bool {
    if s.is_empty() || s.len() > MAX_LEN {
        return false;
    }
    let segments: Vec<&str> = s.split('.').collect();
    if segments.len() < 3 {
        return false;
    }
    segments.iter().all(|seg| {
        seg.starts_with(|c: char| c.is_ascii_lowercase())
            && !seg.ends_with('-')
            && seg
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    })
}

#[cfg(test)]
mod tests {
    use super::is_valid;

    #[test]
    fn accepts_reverse_dns_ids() {
        assert!(is_valid("app.super-stt.voxtral"));
        assert!(is_valid("com.example.whisper"));
        assert!(is_valid("io.a.b.c.d"));
        assert!(is_valid("org.x.qwen3-asr"));
    }

    #[test]
    fn rejects_malformed_ids() {
        assert!(!is_valid(""), "empty");
        assert!(!is_valid("voxtral"), "one segment");
        assert!(!is_valid("app.voxtral"), "two segments");
        assert!(!is_valid("app..voxtral"), "consecutive dots");
        assert!(!is_valid(".app.super-stt.voxtral"), "leading dot");
        assert!(!is_valid("app.super-stt.voxtral."), "trailing dot");
        assert!(
            !is_valid("app.super-stt.3voxtral"),
            "segment starts with a digit"
        );
        assert!(
            !is_valid("app.super-stt.voxtral-"),
            "segment ends with a hyphen"
        );
        assert!(!is_valid("App.Super-STT.Voxtral"), "uppercase");
        assert!(!is_valid("app.super_stt.voxtral"), "underscore");
        assert!(!is_valid("app/super-stt/voxtral"), "path separator");
        assert!(!is_valid(".."), "parent dir");
    }

    #[test]
    fn rejects_an_over_length_id() {
        let long = format!("app.super-stt.{}", "a".repeat(250));
        assert!(!is_valid(&long));
    }

    /// The format rules already imply this, but the path-joining guarantee
    /// must not rest on them alone.
    #[test]
    fn every_valid_id_is_a_safe_path_component() {
        for id in ["app.super-stt.voxtral", "com.example.whisper", "io.a.b.c.d"] {
            assert!(crate::is_safe_component(id), "{id}");
        }
    }
}
