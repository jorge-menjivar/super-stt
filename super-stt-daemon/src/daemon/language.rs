// SPDX-License-Identifier: GPL-3.0-only
//! Resolve the effective transcription language for the active model.

/// Where the resolved language came from — surfaced to clients so the UI can
/// label it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageSource {
    Override,
    Global,
    Default,
}

impl LanguageSource {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Override => "override",
            Self::Global => "global",
            Self::Default => "default",
        }
    }
}

/// The outcome of resolving a language for one transcription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLanguage {
    /// Value to send in the backend `language` field, or `None` to omit it
    /// (the model then uses its `primary_language`). A BCP-47 tag or `"auto"`.
    pub wire: Option<String>,
    /// Where `wire` came from (override / global / model default).
    pub source: LanguageSource,
}

fn has_region(tag: &str) -> bool {
    tag.contains('-')
}

fn base(tag: &str) -> &str {
    tag.split('-').next().unwrap_or(tag)
}

/// Match a chosen tag against a model's `supported_languages` with region rules.
/// Returns the value to send, or `None` to omit.
fn adapt(tag: &str, supported: &[String]) -> Option<String> {
    if tag == "auto" {
        return Some("auto".to_string());
    }
    if supported.iter().any(|s| s == tag) {
        return Some(tag.to_string()); // exact (base or region)
    }
    // No exact match: a region tag falls back to the base language when the
    // model lists it (e.g. es-MX → es), even if the model also carries region
    // variants for that language (e.g. Deepgram's `es` + `es-419`). Only when
    // the base isn't supported either is the language unsupported → omit.
    if has_region(tag) {
        let b = base(tag);
        if supported.iter().any(|s| s == b) {
            return Some(b.to_string());
        }
    }
    None
}

/// Resolve the effective language for the active model. See the spec's
/// resolution pseudocode.
#[must_use]
pub fn resolve_language(
    multilingual: bool,
    override_: Option<&str>,
    global: Option<&str>,
    supported: &[String],
) -> ResolvedLanguage {
    if !multilingual {
        return ResolvedLanguage {
            wire: None,
            source: LanguageSource::Default,
        };
    }
    if let Some(tag) = override_
        && let Some(wire) = adapt(tag, supported)
    {
        return ResolvedLanguage {
            wire: Some(wire),
            source: LanguageSource::Override,
        };
    }
    if let Some(tag) = global
        && let Some(wire) = adapt(tag, supported)
    {
        return ResolvedLanguage {
            wire: Some(wire),
            source: LanguageSource::Global,
        };
    }
    ResolvedLanguage {
        wire: None,
        source: LanguageSource::Default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn non_multilingual_never_sends_language() {
        let r = resolve_language(false, Some("es"), Some("fr"), &s(&["en"]));
        assert_eq!(r.wire, None);
        assert_eq!(r.source, LanguageSource::Default);
    }

    #[test]
    fn override_exact_wins() {
        let r = resolve_language(true, Some("fr"), Some("es"), &s(&["en", "fr", "es"]));
        assert_eq!(r.wire.as_deref(), Some("fr"));
        assert_eq!(r.source, LanguageSource::Override);
    }

    #[test]
    fn override_auto_always_applies() {
        let r = resolve_language(true, Some("auto"), None, &s(&["en"]));
        assert_eq!(r.wire.as_deref(), Some("auto"));
        assert_eq!(r.source, LanguageSource::Override);
    }

    #[test]
    fn override_unsupported_falls_to_global() {
        let r = resolve_language(true, Some("xx"), Some("es"), &s(&["en", "es"]));
        assert_eq!(r.wire.as_deref(), Some("es"));
        assert_eq!(r.source, LanguageSource::Global);
    }

    #[test]
    fn global_exact_when_no_override() {
        let r = resolve_language(true, None, Some("es-419"), &s(&["en", "es-419"]));
        assert_eq!(r.wire.as_deref(), Some("es-419"));
        assert_eq!(r.source, LanguageSource::Global);
    }

    #[test]
    fn global_region_strips_for_region_agnostic_model() {
        // Model lists base `es`, no region variants → strip region.
        let r = resolve_language(true, None, Some("es-MX"), &s(&["en", "es"]));
        assert_eq!(r.wire.as_deref(), Some("es"));
        assert_eq!(r.source, LanguageSource::Global);
    }

    #[test]
    fn global_region_strips_to_base_even_with_region_siblings() {
        // Model lists base `es` AND a region variant (`es-419`), like Deepgram.
        // A picked es-MX strips to base `es` rather than falling back to default.
        let r = resolve_language(true, None, Some("es-MX"), &s(&["en", "es", "es-419"]));
        assert_eq!(r.wire.as_deref(), Some("es"));
        assert_eq!(r.source, LanguageSource::Global);
    }

    #[test]
    fn global_region_falls_back_when_base_unsupported() {
        // Model carries only region variants for `es` (`es-419`, `es-ES`) and no
        // base `es`, and lacks es-MX → the language is unsupported → default.
        let r = resolve_language(true, None, Some("es-MX"), &s(&["en-US", "es-419", "es-ES"]));
        assert_eq!(r.wire, None);
        assert_eq!(r.source, LanguageSource::Default);
    }

    #[test]
    fn global_unrecognized_base_tag_falls_back() {
        let r = resolve_language(true, None, Some("ja"), &s(&["en", "es"]));
        assert_eq!(r.wire, None);
        assert_eq!(r.source, LanguageSource::Default);
    }

    #[test]
    fn unset_is_default() {
        let r = resolve_language(true, None, None, &s(&["en", "es"]));
        assert_eq!(r.wire, None);
        assert_eq!(r.source, LanguageSource::Default);
    }
}
