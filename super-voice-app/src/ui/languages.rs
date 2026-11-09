// SPDX-License-Identifier: GPL-3.0-only
//! Human-friendly names for language tags. The wire form is always the BCP-47
//! tag; names are display-only and composed at render time from a base-language
//! table and a region table, so there is a single source of truth.
//!
//! The tags themselves are the daemon's. The global picker used to offer a
//! curated `GLOBAL_LANGUAGES` const that lived here, which meant this client
//! decided what the daemon accepted — and no other client could know. It is
//! `GET /settings/language/list` now; per-model pickers read
//! `GET /stt/pipeline/{stage}/model/{model}/language/list`.

/// Script subtag (ISO 15924) → localized message ID. Names the `Hans` / `Hant` Chinese
/// distinction (a script, not a region) and the Latin / Cyrillic / Arabic
/// variants some languages carry. Unrecognized scripts keep their raw subtag.
const SCRIPTS: &[(&str, &str)] = &[
    ("Hans", "script-hans"),
    ("Hant", "script-hant"),
    ("Latn", "script-latn"),
    ("Cyrl", "script-cyrl"),
    ("Arab", "script-arab"),
    ("Deva", "script-deva"),
    ("Mong", "script-mong"),
];

/// Base language subtag (ISO 639-1, plus a few ISO 639-3) → localized message ID.
const BASE_LANGUAGES: &[(&str, &str)] = &[
    ("af", "language-af"),
    ("am", "language-am"),
    ("ar", "language-ar"),
    ("as", "language-as"),
    ("az", "language-az"),
    ("ba", "language-ba"),
    ("be", "language-be"),
    ("bg", "language-bg"),
    ("bn", "language-bn"),
    ("bo", "language-bo"),
    ("br", "language-br"),
    ("bs", "language-bs"),
    ("ca", "language-ca"),
    ("cs", "language-cs"),
    ("cy", "language-cy"),
    ("da", "language-da"),
    ("de", "language-de"),
    ("el", "language-el"),
    ("en", "language-en"),
    ("es", "language-es"),
    ("et", "language-et"),
    ("eu", "language-eu"),
    ("fa", "language-fa"),
    ("fi", "language-fi"),
    ("fo", "language-fo"),
    ("fr", "language-fr"),
    ("gl", "language-gl"),
    ("gu", "language-gu"),
    ("ha", "language-ha"),
    ("haw", "language-haw"),
    ("he", "language-he"),
    ("hi", "language-hi"),
    ("hr", "language-hr"),
    ("ht", "language-ht"),
    ("hu", "language-hu"),
    ("hy", "language-hy"),
    ("id", "language-id"),
    ("is", "language-is"),
    ("it", "language-it"),
    ("ja", "language-ja"),
    ("jw", "language-jw"),
    ("ka", "language-ka"),
    ("kk", "language-kk"),
    ("km", "language-km"),
    ("kn", "language-kn"),
    ("ko", "language-ko"),
    ("la", "language-la"),
    ("lb", "language-lb"),
    ("ln", "language-ln"),
    ("lo", "language-lo"),
    ("lt", "language-lt"),
    ("lv", "language-lv"),
    ("mg", "language-mg"),
    ("mi", "language-mi"),
    ("mk", "language-mk"),
    ("ml", "language-ml"),
    ("mn", "language-mn"),
    ("mr", "language-mr"),
    ("ms", "language-ms"),
    ("mt", "language-mt"),
    ("my", "language-my"),
    ("ne", "language-ne"),
    ("nl", "language-nl"),
    ("nn", "language-nn"),
    ("no", "language-no"),
    ("oc", "language-oc"),
    ("pa", "language-pa"),
    ("pl", "language-pl"),
    ("ps", "language-ps"),
    ("pt", "language-pt"),
    ("ro", "language-ro"),
    ("ru", "language-ru"),
    ("sa", "language-sa"),
    ("sd", "language-sd"),
    ("si", "language-si"),
    ("sk", "language-sk"),
    ("sl", "language-sl"),
    ("sn", "language-sn"),
    ("so", "language-so"),
    ("sq", "language-sq"),
    ("sr", "language-sr"),
    ("su", "language-su"),
    ("sv", "language-sv"),
    ("sw", "language-sw"),
    ("ta", "language-ta"),
    ("te", "language-te"),
    ("tg", "language-tg"),
    ("th", "language-th"),
    ("tk", "language-tk"),
    ("tl", "language-tl"),
    ("tr", "language-tr"),
    ("tt", "language-tt"),
    ("uk", "language-uk"),
    ("ur", "language-ur"),
    ("uz", "language-uz"),
    ("vi", "language-vi"),
    ("yi", "language-yi"),
    ("yo", "language-yo"),
    ("yue", "language-yue"),
    ("zh", "language-zh"),
];

/// Region subtag → localized message ID: ISO 3166-1 alpha-2 country codes and UN M.49
/// area codes (`419` = Latin America).
const REGIONS: &[(&str, &str)] = &[
    ("AE", "region-ae"),
    ("AR", "region-ar"),
    ("AT", "region-at"),
    ("AU", "region-au"),
    ("BD", "region-bd"),
    ("BE", "region-be"),
    ("BG", "region-bg"),
    ("BO", "region-bo"),
    ("BR", "region-br"),
    ("BY", "region-by"),
    ("CA", "region-ca"),
    ("CH", "region-ch"),
    ("CL", "region-cl"),
    ("CN", "region-cn"),
    ("CO", "region-co"),
    ("CR", "region-cr"),
    ("CU", "region-cu"),
    ("CY", "region-cy"),
    ("CZ", "region-cz"),
    ("DE", "region-de"),
    ("DK", "region-dk"),
    ("DO", "region-do"),
    ("EC", "region-ec"),
    ("EE", "region-ee"),
    ("EG", "region-eg"),
    ("ES", "region-es"),
    ("FI", "region-fi"),
    ("FR", "region-fr"),
    ("GB", "region-gb"),
    ("GE", "region-ge"),
    ("GR", "region-gr"),
    ("GT", "region-gt"),
    ("HK", "region-hk"),
    ("HR", "region-hr"),
    ("HU", "region-hu"),
    ("ID", "region-id"),
    ("IE", "region-ie"),
    ("IL", "region-il"),
    ("IN", "region-in"),
    ("IQ", "region-iq"),
    ("IR", "region-ir"),
    ("IS", "region-is"),
    ("IT", "region-it"),
    ("JP", "region-jp"),
    ("KE", "region-ke"),
    ("KR", "region-kr"),
    ("KZ", "region-kz"),
    ("LK", "region-lk"),
    ("LT", "region-lt"),
    ("LU", "region-lu"),
    ("LV", "region-lv"),
    ("MA", "region-ma"),
    ("MX", "region-mx"),
    ("MY", "region-my"),
    ("NG", "region-ng"),
    ("NL", "region-nl"),
    ("NO", "region-no"),
    ("NP", "region-np"),
    ("NZ", "region-nz"),
    ("PA", "region-pa"),
    ("PE", "region-pe"),
    ("PH", "region-ph"),
    ("PK", "region-pk"),
    ("PL", "region-pl"),
    ("PT", "region-pt"),
    ("PY", "region-py"),
    ("RO", "region-ro"),
    ("RS", "region-rs"),
    ("RU", "region-ru"),
    ("SA", "region-sa"),
    ("SE", "region-se"),
    ("SG", "region-sg"),
    ("SI", "region-si"),
    ("SK", "region-sk"),
    ("SV", "region-sv"),
    ("TH", "region-th"),
    ("TR", "region-tr"),
    ("TW", "region-tw"),
    ("UA", "region-ua"),
    ("US", "region-us"),
    ("UY", "region-uy"),
    ("VE", "region-ve"),
    ("VN", "region-vn"),
    ("ZA", "region-za"),
    ("419", "region-419"),
];

/// Case-insensitive lookup in a `(code, name)` table.
fn lookup(table: &[(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    table
        .iter()
        .find(|&&(code, _)| code.eq_ignore_ascii_case(key))
        .map(|&(_, name)| name)
}

/// Whether a BCP-47 subtag has script shape: 4 ASCII letters (e.g. `Hans`).
fn is_script(sub: &str) -> bool {
    sub.len() == 4 && sub.chars().all(|c| c.is_ascii_alphabetic())
}

/// Whether a BCP-47 subtag has region shape: a 2-letter country code or a
/// 3-digit UN M.49 area code (e.g. `US`, `419`).
fn is_region(sub: &str) -> bool {
    (sub.len() == 2 && sub.chars().all(|c| c.is_ascii_alphabetic()))
        || (sub.len() == 3 && sub.chars().all(|c| c.is_ascii_digit()))
}

/// Friendly display name for any BCP-47 tag.
///
/// `auto` becomes "Auto-detect". Otherwise the tag is split into subtags, each
/// classified by shape: the first is the language, a 4-letter subtag is a
/// script, and a 2-letter or 3-digit subtag is a region. Recognized subtags
/// render via their table; unrecognized ones keep their raw code (regions
/// upper-cased). The result is "Language (qualifier, …)" — so `en-US` renders
/// "English (United States)", `zh-Hans` renders "Chinese (Simplified)",
/// `zh-Hans-CN` renders "Chinese (Simplified, China)", `en-XYZ` renders
/// "English (XYZ)", and a bare unknown `xx` renders "xx".
#[must_use]
pub fn friendly_name(tag: &str) -> String {
    if tag.eq_ignore_ascii_case("auto") {
        return crate::fl!("auto-detect");
    }
    let mut subtags = tag.split('-');
    let lang = subtags.next().unwrap_or(tag);
    let name = match lookup(BASE_LANGUAGES, lang) {
        Some(n) => crate::i18n::LANGUAGE_LOADER.get(n),
        None => lang.to_string(),
    };
    let mut qualifiers: Vec<String> = Vec::new();
    for sub in subtags {
        if sub.is_empty() {
            continue; // tolerate a trailing/double hyphen without empty parens
        }
        if is_script(sub) {
            qualifiers.push(match lookup(SCRIPTS, sub) {
                Some(n) => crate::i18n::LANGUAGE_LOADER.get(n),
                None => sub.to_string(),
            });
        } else if is_region(sub) {
            qualifiers.push(match lookup(REGIONS, sub) {
                Some(n) => crate::i18n::LANGUAGE_LOADER.get(n),
                None => sub.to_uppercase(),
            });
        } else {
            qualifiers.push(sub.to_string());
        }
    }
    if qualifiers.is_empty() {
        name
    } else {
        format!("{name} ({})", qualifiers.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::friendly_name;

    #[test]
    fn auto_and_bare_language() {
        assert_eq!(friendly_name("auto"), "Auto-detect");
        assert_eq!(friendly_name("en"), "English");
        assert_eq!(friendly_name("zh"), "Chinese");
    }

    #[test]
    fn language_region() {
        assert_eq!(friendly_name("en-US"), "English (United States)");
        assert_eq!(friendly_name("es-419"), "Spanish (Latin America)");
        assert_eq!(friendly_name("zh-CN"), "Chinese (China)");
        assert_eq!(friendly_name("zh-TW"), "Chinese (Taiwan)");
    }

    #[test]
    fn language_script() {
        assert_eq!(friendly_name("zh-Hans"), "Chinese (Simplified)");
        assert_eq!(friendly_name("zh-Hant"), "Chinese (Traditional)");
        assert_eq!(friendly_name("sr-Latn"), "Serbian (Latin)");
    }

    #[test]
    fn language_script_region() {
        // Three subtags (two hyphens) — the script must not be mistaken for the
        // region, and both qualifiers appear.
        assert_eq!(friendly_name("zh-Hans-CN"), "Chinese (Simplified, China)");
        assert_eq!(friendly_name("zh-Hant-TW"), "Chinese (Traditional, Taiwan)");
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(friendly_name("EN-us"), "English (United States)");
        assert_eq!(friendly_name("zh-hans"), "Chinese (Simplified)");
    }

    #[test]
    fn unknown_subtags_kept_raw() {
        assert_eq!(friendly_name("en-XYZ"), "English (XYZ)");
        assert_eq!(friendly_name("xx"), "xx");
    }

    #[test]
    fn empty_subtags_dropped() {
        assert_eq!(friendly_name("en-"), "English");
        assert_eq!(friendly_name("en--US"), "English (United States)");
    }
}
