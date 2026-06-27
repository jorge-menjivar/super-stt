// SPDX-License-Identifier: GPL-3.0-only
//! Language tags for the pickers plus human-friendly names. The wire form is
//! always the BCP-47 tag; names are display-only and composed at render time
//! from a base-language table and a region table, so there is a single source
//! of truth. The global Primary Language picker offers the curated,
//! region-qualified [`GLOBAL_LANGUAGES`]; per-model pickers reuse
//! [`friendly_name`] for each model's own `supported_languages`, which are
//! usually base ISO 639-1 codes (e.g. `es`).

/// Tags offered by the global Primary Language picker. Region-qualified, with
/// one exception: Chinese uses the ISO 15924 script subtags `zh-Hans` /
/// `zh-Hant`, since script — not region — is what distinguishes Simplified from
/// Traditional. A region is a country code, or UN M.49 for a multi-country
/// region (`es-419`). Display order is alphabetical by name and is applied by
/// the picker; every subtag here resolves through the tables below.
pub const GLOBAL_LANGUAGES: &[&str] = &[
    "af-ZA", "ar-EG", "ar-SA", "bn-BD", "bg-BG", "ca-ES", "zh-Hans", "zh-Hant", "hr-HR", "cs-CZ",
    "da-DK", "nl-NL", "en-AU", "en-CA", "en-IN", "en-GB", "en-US", "et-EE", "fi-FI", "fr-BE",
    "fr-CA", "fr-FR", "de-AT", "de-DE", "de-CH", "el-GR", "he-IL", "hi-IN", "hu-HU", "id-ID",
    "it-IT", "ja-JP", "ko-KR", "ms-MY", "no-NO", "fa-IR", "pl-PL", "pt-BR", "pt-PT", "ro-RO",
    "ru-RU", "sk-SK", "es-419", "es-MX", "es-ES", "es-US", "sw-KE", "sv-SE", "tl-PH", "ta-IN",
    "te-IN", "th-TH", "tr-TR", "uk-UA", "ur-PK", "vi-VN", "cy-GB",
];

/// Script subtag (ISO 15924) → display name. Names the `Hans` / `Hant` Chinese
/// distinction (a script, not a region) and the Latin / Cyrillic / Arabic
/// variants some languages carry. Unrecognized scripts keep their raw subtag.
const SCRIPTS: &[(&str, &str)] = &[
    ("Hans", "Simplified"),
    ("Hant", "Traditional"),
    ("Latn", "Latin"),
    ("Cyrl", "Cyrillic"),
    ("Arab", "Arabic"),
    ("Deva", "Devanagari"),
    ("Mong", "Mongolian"),
];

/// Base language subtag (ISO 639-1, plus a few ISO 639-3) → English name.
const BASE_LANGUAGES: &[(&str, &str)] = &[
    ("af", "Afrikaans"),
    ("am", "Amharic"),
    ("ar", "Arabic"),
    ("as", "Assamese"),
    ("az", "Azerbaijani"),
    ("ba", "Bashkir"),
    ("be", "Belarusian"),
    ("bg", "Bulgarian"),
    ("bn", "Bengali"),
    ("bo", "Tibetan"),
    ("br", "Breton"),
    ("bs", "Bosnian"),
    ("ca", "Catalan"),
    ("cs", "Czech"),
    ("cy", "Welsh"),
    ("da", "Danish"),
    ("de", "German"),
    ("el", "Greek"),
    ("en", "English"),
    ("es", "Spanish"),
    ("et", "Estonian"),
    ("eu", "Basque"),
    ("fa", "Persian"),
    ("fi", "Finnish"),
    ("fo", "Faroese"),
    ("fr", "French"),
    ("gl", "Galician"),
    ("gu", "Gujarati"),
    ("ha", "Hausa"),
    ("haw", "Hawaiian"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("hr", "Croatian"),
    ("ht", "Haitian Creole"),
    ("hu", "Hungarian"),
    ("hy", "Armenian"),
    ("id", "Indonesian"),
    ("is", "Icelandic"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("jw", "Javanese"),
    ("ka", "Georgian"),
    ("kk", "Kazakh"),
    ("km", "Khmer"),
    ("kn", "Kannada"),
    ("ko", "Korean"),
    ("la", "Latin"),
    ("lb", "Luxembourgish"),
    ("ln", "Lingala"),
    ("lo", "Lao"),
    ("lt", "Lithuanian"),
    ("lv", "Latvian"),
    ("mg", "Malagasy"),
    ("mi", "Maori"),
    ("mk", "Macedonian"),
    ("ml", "Malayalam"),
    ("mn", "Mongolian"),
    ("mr", "Marathi"),
    ("ms", "Malay"),
    ("mt", "Maltese"),
    ("my", "Burmese"),
    ("ne", "Nepali"),
    ("nl", "Dutch"),
    ("nn", "Norwegian Nynorsk"),
    ("no", "Norwegian"),
    ("oc", "Occitan"),
    ("pa", "Punjabi"),
    ("pl", "Polish"),
    ("ps", "Pashto"),
    ("pt", "Portuguese"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("sa", "Sanskrit"),
    ("sd", "Sindhi"),
    ("si", "Sinhala"),
    ("sk", "Slovak"),
    ("sl", "Slovenian"),
    ("sn", "Shona"),
    ("so", "Somali"),
    ("sq", "Albanian"),
    ("sr", "Serbian"),
    ("su", "Sundanese"),
    ("sv", "Swedish"),
    ("sw", "Swahili"),
    ("ta", "Tamil"),
    ("te", "Telugu"),
    ("tg", "Tajik"),
    ("th", "Thai"),
    ("tk", "Turkmen"),
    ("tl", "Tagalog"),
    ("tr", "Turkish"),
    ("tt", "Tatar"),
    ("uk", "Ukrainian"),
    ("ur", "Urdu"),
    ("uz", "Uzbek"),
    ("vi", "Vietnamese"),
    ("yi", "Yiddish"),
    ("yo", "Yoruba"),
    ("yue", "Cantonese"),
    ("zh", "Chinese"),
];

/// Region subtag → English name: ISO 3166-1 alpha-2 country codes and UN M.49
/// area codes (`419` = Latin America).
const REGIONS: &[(&str, &str)] = &[
    ("AE", "United Arab Emirates"),
    ("AR", "Argentina"),
    ("AT", "Austria"),
    ("AU", "Australia"),
    ("BD", "Bangladesh"),
    ("BE", "Belgium"),
    ("BG", "Bulgaria"),
    ("BO", "Bolivia"),
    ("BR", "Brazil"),
    ("BY", "Belarus"),
    ("CA", "Canada"),
    ("CH", "Switzerland"),
    ("CL", "Chile"),
    ("CN", "China"),
    ("CO", "Colombia"),
    ("CR", "Costa Rica"),
    ("CU", "Cuba"),
    ("CY", "Cyprus"),
    ("CZ", "Czechia"),
    ("DE", "Germany"),
    ("DK", "Denmark"),
    ("DO", "Dominican Republic"),
    ("EC", "Ecuador"),
    ("EE", "Estonia"),
    ("EG", "Egypt"),
    ("ES", "Spain"),
    ("FI", "Finland"),
    ("FR", "France"),
    ("GB", "United Kingdom"),
    ("GE", "Georgia"),
    ("GR", "Greece"),
    ("GT", "Guatemala"),
    ("HK", "Hong Kong"),
    ("HR", "Croatia"),
    ("HU", "Hungary"),
    ("ID", "Indonesia"),
    ("IE", "Ireland"),
    ("IL", "Israel"),
    ("IN", "India"),
    ("IQ", "Iraq"),
    ("IR", "Iran"),
    ("IS", "Iceland"),
    ("IT", "Italy"),
    ("JP", "Japan"),
    ("KE", "Kenya"),
    ("KR", "South Korea"),
    ("KZ", "Kazakhstan"),
    ("LK", "Sri Lanka"),
    ("LT", "Lithuania"),
    ("LU", "Luxembourg"),
    ("LV", "Latvia"),
    ("MA", "Morocco"),
    ("MX", "Mexico"),
    ("MY", "Malaysia"),
    ("NG", "Nigeria"),
    ("NL", "Netherlands"),
    ("NO", "Norway"),
    ("NP", "Nepal"),
    ("NZ", "New Zealand"),
    ("PA", "Panama"),
    ("PE", "Peru"),
    ("PH", "Philippines"),
    ("PK", "Pakistan"),
    ("PL", "Poland"),
    ("PT", "Portugal"),
    ("PY", "Paraguay"),
    ("RO", "Romania"),
    ("RS", "Serbia"),
    ("RU", "Russia"),
    ("SA", "Saudi Arabia"),
    ("SE", "Sweden"),
    ("SG", "Singapore"),
    ("SI", "Slovenia"),
    ("SK", "Slovakia"),
    ("SV", "El Salvador"),
    ("TH", "Thailand"),
    ("TR", "Turkey"),
    ("TW", "Taiwan"),
    ("UA", "Ukraine"),
    ("US", "United States"),
    ("UY", "Uruguay"),
    ("VE", "Venezuela"),
    ("VN", "Vietnam"),
    ("ZA", "South Africa"),
    ("419", "Latin America"),
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
        return "Auto-detect".to_string();
    }
    let mut subtags = tag.split('-');
    let lang = subtags.next().unwrap_or(tag);
    let name = match lookup(BASE_LANGUAGES, lang) {
        Some(n) => n.to_string(),
        None => lang.to_string(),
    };
    let mut qualifiers: Vec<String> = Vec::new();
    for sub in subtags {
        if sub.is_empty() {
            continue; // tolerate a trailing/double hyphen without empty parens
        }
        if is_script(sub) {
            qualifiers.push(match lookup(SCRIPTS, sub) {
                Some(n) => n.to_string(),
                None => sub.to_string(),
            });
        } else if is_region(sub) {
            qualifiers.push(match lookup(REGIONS, sub) {
                Some(n) => n.to_string(),
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
