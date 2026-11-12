// SPDX-License-Identifier: GPL-3.0-only
//! Super Voice's COSMIC keyboard shortcuts: which there are, whether COSMIC's
//! custom-shortcuts file has each one, and adding one.
//!
//! The daemon adds and removes them, for the app's Shortcuts page, and the
//! installer keeps the ones installed pointing at its `voice`.
//!
//! The file is a RON map from a binding, `(modifiers: [..], key: "..",
//! description: Some(".."))`, to an action, `Spawn("..")` for a command.
//! It is read here binding by binding rather than parsed in full: only the
//! keys, the name and the command matter, and text inside a name or a
//! command must not be mistaken for a binding.

use std::fmt::Write as _;
use std::path::PathBuf;

/// One of Super Voice's COSMIC shortcuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcut {
    /// The modifiers, as COSMIC names them.
    pub modifiers: &'static [&'static str],
    /// The key, as an XKB keysym name.
    pub key: &'static str,
    /// Its name in COSMIC's keyboard settings.
    pub description: &'static str,
    /// What it runs `voice` with.
    pub args: &'static str,
    /// The words that make a shortcut running `voice` this one, whichever
    /// `voice` it runs and whatever else it passes.
    pub marker: &'static [&'static str],
}

impl Shortcut {
    /// The keys as people write them, like `Super+Shift+Space`.
    #[must_use]
    pub fn keys(&self) -> String {
        keys_text(self.modifiers, self.key)
    }
}

/// `modifiers` and `key` as people write them: `Super+Shift+Space`.
fn keys_text(modifiers: &[&str], key: &str) -> String {
    let mut key = key.to_string();
    if let Some(first) = key.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    modifiers
        .iter()
        .copied()
        .chain([key.as_str()])
        .collect::<Vec<_>>()
        .join("+")
}

/// Super+Space dictates. COSMIC binds it to switch the input source by
/// default, which dictation has always taken over.
pub const DICTATE: Shortcut = Shortcut {
    modifiers: &["Super"],
    key: "space",
    description: "Super Voice",
    args: "record --write",
    marker: &["record"],
};

/// COSMIC's custom-shortcuts file, under `$XDG_CONFIG_HOME`.
#[must_use]
pub fn file() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("cosmic/com.system76.CosmicSettings.Shortcuts/v1/custom"))
}

/// Where a shortcut stands in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The file has it, on `keys`: its own keys, or others someone bound it to.
    Installed { keys: String },
    /// The file does not have it, and its keys are free.
    Missing,
    /// The file does not have it, and something else holds its keys.
    Taken,
}

/// Where `shortcut` stands in `content`, the file's text.
#[must_use]
pub fn status(content: &str, shortcut: &Shortcut) -> Status {
    if is_installed(content, shortcut) {
        let keys = entries(content)
            .into_iter()
            .find(|entry| entry.is(shortcut))
            .and_then(|entry| entry.key.map(|key| keys_text(&entry.modifiers, key)))
            .unwrap_or_else(|| shortcut.keys());
        Status::Installed { keys }
    } else if is_taken(content, shortcut) {
        Status::Taken
    } else {
        Status::Missing
    }
}

/// Add `shortcut`, running `command`, to the file. `Ok(false)` when it was
/// not added: the file has it already, or its keys are taken.
///
/// # Errors
///
/// When there is no config directory, or the file cannot be written.
pub fn add(shortcut: &Shortcut, command: &str) -> std::io::Result<bool> {
    let path = file().ok_or_else(|| std::io::Error::other("no config directory"))?;
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let Some(updated) = with_shortcut(&content, shortcut, command) else {
        return Ok(false);
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, updated)?;
    Ok(true)
}

/// The argument words of each shortcut in `content` that runs `voice`, from
/// any place.
fn voice_commands(content: &str) -> impl Iterator<Item = Vec<&str>> {
    content.split("Spawn(\"").skip(1).filter_map(|rest| {
        let command = rest.split('"').next().unwrap_or_default();
        let mut words = command.split_whitespace();
        let program = words.next().unwrap_or_default();
        (program == "voice" || program.ends_with("/voice")).then(|| words.collect())
    })
}

/// Whether `command`'s words make it `shortcut`: it runs `voice` with every
/// word of the shortcut's [`marker`](Shortcut::marker).
fn runs(words: &[&str], shortcut: &Shortcut) -> bool {
    shortcut.marker.iter().all(|m| words.contains(m))
}

/// Whether `shortcut` is in `content` already: an entry with its name, or
/// any shortcut that runs `voice` with its [`marker`](Shortcut::marker),
/// whatever it is called.
#[must_use]
pub fn is_installed(content: &str, shortcut: &Shortcut) -> bool {
    content.contains(&format!("description: Some(\"{}\")", shortcut.description))
        || voice_commands(content).any(|words| runs(&words, shortcut))
}

/// Split `text` at each `separator` outside strings and brackets.
fn split_top_level(text: &str, separator: char) -> Vec<&str> {
    split_top_level_ranges(text, separator)
        .into_iter()
        .map(|range| &text[range])
        .collect()
}

/// Where [`split_top_level`]'s parts are in `text`.
fn split_top_level_ranges(text: &str, separator: char) -> Vec<std::ops::Range<usize>> {
    let mut parts = Vec::new();
    let (mut depth, mut in_string, mut escaped, mut start) = (0usize, false, false, 0);
    for (at, c) in text.char_indices() {
        if in_string {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ if c == separator && depth == 0 => {
                parts.push(start..at);
                start = at + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(start..text.len());
    parts
}

/// One entry of the file: a binding and what it does.
struct Entry<'a> {
    modifiers: Vec<&'a str>,
    /// `None` for a binding with no key, such as Super alone.
    key: Option<&'a str>,
    description: Option<&'a str>,
    /// What follows the binding: `Spawn("..")`, `Disable`, and so on.
    action: &'a str,
}

impl Entry<'_> {
    /// Whether this entry is `shortcut`: its name, or a command running
    /// `voice` with its marker.
    fn is(&self, shortcut: &Shortcut) -> bool {
        if self.description == Some(shortcut.description) {
            return true;
        }
        voice_commands(self.action).any(|words| runs(&words, shortcut))
    }
}

/// Where the map's body is in `content`: between its outer braces. `None`
/// for a file that is not a map.
fn body_range(content: &str) -> Option<std::ops::Range<usize>> {
    let start = content.len() - content.trim_start().len();
    let end = content.trim_end().len();
    (content[start..end].starts_with('{') && content[start..end].ends_with('}') && end > start + 1)
        .then(|| start + 1..end - 1)
}

/// Every entry in `content`, with where it is. A file that is not a map has
/// none.
fn entries_at(content: &str) -> Vec<(std::ops::Range<usize>, Entry<'_>)> {
    let Some(body) = body_range(content) else {
        return Vec::new();
    };
    split_top_level_ranges(&content[body.clone()], ',')
        .into_iter()
        .filter_map(|range| {
            let range = body.start + range.start..body.start + range.end;
            Some((range.clone(), entry(&content[range])?))
        })
        .collect()
}

/// Every entry in `content`. A file that is not a map has none.
fn entries(content: &str) -> Vec<Entry<'_>> {
    entries_at(content)
        .into_iter()
        .map(|(_, entry)| entry)
        .collect()
}

/// One entry's text read as a binding and its action, or `None` when it is
/// not one, like the space after the last comma.
fn entry(entry: &str) -> Option<Entry<'_>> {
    let binding = split_top_level(entry, ':').into_iter().next()?;
    let action = entry.get(binding.len() + 1..).unwrap_or_default().trim();
    let fields = binding.trim().strip_prefix('(')?.strip_suffix(')')?;
    let mut parsed = Entry {
        modifiers: Vec::new(),
        key: None,
        description: None,
        action,
    };
    for field in split_top_level(fields, ',') {
        let Some((name, value)) = field.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim() {
            "modifiers" => {
                let list = value.trim_start_matches('[').trim_end_matches(']');
                parsed.modifiers = list
                    .split(',')
                    .map(str::trim)
                    .filter(|m| !m.is_empty())
                    .collect();
            }
            "key" => parsed.key = Some(value.trim_matches('"')),
            "description" => {
                parsed.description = value
                    .strip_prefix("Some(")
                    .and_then(|v| v.strip_suffix(')'))
                    .map(|v| v.trim().trim_matches('"'));
            }
            _ => {}
        }
    }
    Some(parsed)
}

/// Whether `content` binds `shortcut`'s keys to anything already, our own
/// shortcut included. The modifiers match in any order.
#[must_use]
pub fn is_taken(content: &str, shortcut: &Shortcut) -> bool {
    entries(content).into_iter().any(|entry| {
        entry
            .key
            .is_some_and(|key| key.eq_ignore_ascii_case(shortcut.key))
            && entry.modifiers.len() == shortcut.modifiers.len()
            && shortcut
                .modifiers
                .iter()
                .all(|m| entry.modifiers.contains(m))
    })
}

/// The `Spawn(...)` entry for `shortcut` running `command`, RON-shaped like
/// the rest of the COSMIC shortcuts file.
fn shortcut_entry(shortcut: &Shortcut, command: &str) -> String {
    let modifiers = shortcut.modifiers.iter().fold(String::new(), |mut out, m| {
        let _ = writeln!(out, "            {m},");
        out
    });
    format!(
        "    (\n        modifiers: [\n{modifiers}        ],\n        key: \"{}\",\n        description: Some(\"{}\"),\n    ): Spawn(\"{command}\"),\n",
        shortcut.key, shortcut.description
    )
}

/// `content` (the COSMIC custom-shortcuts file's current text) with
/// `shortcut` added to run `command`, or `None` when it is installed already
/// (see [`is_installed`]) or its keys are taken (see [`is_taken`]). Empty or
/// `{}`-only content gets the full-file template; otherwise the entry is
/// inserted before the final closing brace.
#[must_use]
pub fn with_shortcut(content: &str, shortcut: &Shortcut, command: &str) -> Option<String> {
    if is_installed(content, shortcut) || is_taken(content, shortcut) {
        return None;
    }

    let entry = shortcut_entry(shortcut, command);
    let trimmed = content.trim();
    if trimmed.is_empty() || trimmed == "{}" {
        return Some(format!("{{\n{entry}}}\n"));
    }

    // Everything up to the final `}`, then our entry and a fresh close. The
    // last entry needs its comma, which a hand-edited file may have left off.
    let head = content[..content.rfind('}')?].trim_end();
    let comma = if head.ends_with(',') || head.ends_with('{') {
        ""
    } else {
        ","
    };
    Some(format!("{head}{comma}\n{entry}}}\n"))
}

/// `content` without `shortcut`: every entry that is it, with the comma
/// after each. `None` when there is none to remove.
#[must_use]
pub fn without_shortcut(content: &str, shortcut: &Shortcut) -> Option<String> {
    let doomed: Vec<std::ops::Range<usize>> = entries_at(content)
        .into_iter()
        .filter(|(_, entry)| entry.is(shortcut))
        .map(|(range, _)| {
            let comma = usize::from(content[range.end..].starts_with(','));
            range.start..range.end + comma
        })
        .collect();
    if doomed.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(content.len());
    let mut at = 0;
    for range in doomed {
        out.push_str(&content[at..range.start]);
        at = range.end;
    }
    out.push_str(&content[at..]);
    Some(out)
}

/// Remove `shortcut` from the file. `Ok(false)` when the file did not have
/// it.
///
/// Removing Super+Space hands it back to COSMIC's own binding for those keys,
/// switching the input source.
///
/// # Errors
///
/// When there is no config directory, or the file cannot be written.
pub fn remove(shortcut: &Shortcut) -> std::io::Result<bool> {
    let path = file().ok_or_else(|| std::io::Error::other("no config directory"))?;
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Ok(false);
    };
    let Some(updated) = without_shortcut(&content, shortcut) else {
        return Ok(false);
    };
    std::fs::write(&path, updated)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A second shortcut, on Super+Shift+Space, whose marker has two words:
    /// the rules must keep two of Super Voice's shortcuts in one file apart.
    const READ_ALOUD: Shortcut = Shortcut {
        modifiers: &["Super", "Shift"],
        key: "space",
        description: "Super Voice: read aloud",
        args: "speak --read",
        marker: &["speak", "--read"],
    };

    const RECORD_CMD: &str = "/usr/local/bin/voice record --write";
    const SPEAK_CMD: &str = "/usr/local/bin/voice speak --read";

    /// The file as COSMIC writes it, with Super Voice's first shortcut and a
    /// system one switched off.
    const WITH_RECORD: &str = "{\n    (\n        modifiers: [\n            Super,\n            Ctrl,\n        ],\n        key: \"1\",\n    ): Disable,\n    (\n        modifiers: [\n            Super,\n        ],\n        key: \"space\",\n        description: Some(\"Super Voice\"),\n    ): Spawn(\"/usr/local/bin/voice record --write\"),\n}";

    /// A shortcut that runs `voice` with a shortcut's marker counts as that
    /// one, whatever it is called, so no second one is offered beside it. It
    /// does not count as the other.
    #[test]
    fn a_shortcut_running_voice_is_installed() {
        let c = "{\n    (\n        modifiers: [\n            Ctrl,\n        ],\n        key: \"r\",\n        description: Some(\"Dictate\"),\n    ): Spawn(\"voice record --write\"),\n}";
        assert!(is_installed(c, &DICTATE));
        assert!(with_shortcut(c, &DICTATE, RECORD_CMD).is_none());
        assert!(!is_installed(c, &READ_ALOUD));

        let c = r#"{ (key: "s"): Spawn("/opt/bin/voice speak --read"), }"#;
        assert!(is_installed(c, &READ_ALOUD));
        assert!(!is_installed(c, &DICTATE));

        // `speak` alone is someone's own binding, not the read-aloud one.
        let c = r#"{ (key: "s"): Spawn("voice speak stop"), }"#;
        assert!(!is_installed(c, &READ_ALOUD));

        for other in [r#"{ (key: "v"): Spawn("/usr/bin/nvoice record"), }"#, "{}"] {
            assert!(!is_installed(other, &DICTATE), "{other}");
            assert!(!is_installed(other, &READ_ALOUD), "{other}");
        }
    }

    /// Each shortcut is known by its own name. "Super Voice" is a prefix of
    /// the second one's, which must not count as the first.
    #[test]
    fn each_shortcut_is_known_by_its_own_name() {
        let read_only = "{\n    (\n        modifiers: [\n            Ctrl,\n        ],\n        key: \"s\",\n        description: Some(\"Super Voice: read aloud\"),\n    ): Spawn(\"other\"),\n}";
        assert!(is_installed(read_only, &READ_ALOUD));
        assert!(!is_installed(read_only, &DICTATE));
        assert!(is_installed(
            r#"{ (description: Some("Super Voice")): Spawn("x"), }"#,
            &DICTATE
        ));
    }

    /// Keys bound to anything else are left alone, whatever order the
    /// modifiers are written in. Super+Space taken is no reason to skip
    /// Super+Shift+Space.
    #[test]
    fn a_taken_binding_is_left_alone() {
        let taken = "{\n    (\n        modifiers: [\n            Shift,\n            Super,\n        ],\n        key: \"space\",\n        description: Some(\"Other\"),\n    ): Spawn(\"x\"),\n}";
        assert!(is_taken(taken, &READ_ALOUD));
        assert!(with_shortcut(taken, &READ_ALOUD, SPEAK_CMD).is_none());
        assert!(!is_taken(taken, &DICTATE));

        let disabled = r#"{ (modifiers: [Super], key: "space"): Disable, }"#;
        assert!(is_taken(disabled, &DICTATE));
        assert!(!is_taken(disabled, &READ_ALOUD));

        // Text that only looks like a binding, in a name or a command, is not
        // one.
        let lookalike = r#"{ (modifiers: [Ctrl], key: "x", description: Some("key: \"space\", Super")): Spawn("key: space"), }"#;
        assert!(!is_taken(lookalike, &DICTATE));
        assert!(!is_taken("", &DICTATE));
    }

    /// What the Shortcuts page shows for each: the keys it is on, missing, or
    /// blocked by another binding.
    #[test]
    fn status_names_the_keys_a_shortcut_is_on() {
        assert_eq!(
            status(WITH_RECORD, &DICTATE),
            Status::Installed {
                keys: "Super+Space".to_string()
            }
        );
        assert_eq!(status(WITH_RECORD, &READ_ALOUD), Status::Missing);
        assert_eq!(status("", &DICTATE), Status::Missing);

        // Bound to other keys by hand, it is installed on those.
        let moved = r#"{ (modifiers: [Ctrl, Alt], key: "r"): Spawn("voice speak --read"), }"#;
        assert_eq!(
            status(moved, &READ_ALOUD),
            Status::Installed {
                keys: "Ctrl+Alt+R".to_string()
            }
        );

        let taken = r#"{ (modifiers: [Super, Shift], key: "space"): Spawn("x"), }"#;
        assert_eq!(status(taken, &READ_ALOUD), Status::Taken);
    }

    /// The second shortcut goes after everything else in a file that has the
    /// first, and is not offered again.
    #[test]
    fn the_second_shortcut_joins_the_first() {
        assert!(with_shortcut(WITH_RECORD, &DICTATE, RECORD_CMD).is_none());
        let both = with_shortcut(WITH_RECORD, &READ_ALOUD, SPEAK_CMD).expect("offered");
        assert!(
            both.starts_with(WITH_RECORD.trim_end_matches('}')),
            "{both}"
        );
        assert!(
            both.contains("        modifiers: [\n            Super,\n            Shift,\n        ],\n        key: \"space\",\n        description: Some(\"Super Voice: read aloud\"),\n    ): Spawn(\"/usr/local/bin/voice speak --read\"),\n}"),
            "{both}"
        );
        assert_eq!(entries(&both).len(), 3, "{both}");
        assert!(is_taken(&both, &READ_ALOUD));
        assert!(with_shortcut(&both, &READ_ALOUD, SPEAK_CMD).is_none());
        assert!(with_shortcut(&both, &DICTATE, RECORD_CMD).is_none());
    }

    #[test]
    fn add_writes_full_file_when_empty_and_inserts_before_close_otherwise() {
        let fresh = with_shortcut("", &DICTATE, RECORD_CMD).unwrap();
        assert!(fresh.starts_with('{'));
        assert!(fresh.contains("Super Voice"));
        assert!(fresh.trim_end().ends_with('}'));
        assert_eq!(
            status(&fresh, &DICTATE),
            Status::Installed {
                keys: "Super+Space".to_string()
            }
        );

        // The literal `{}`-only case (not just a truly empty file) also gets
        // the full-file template.
        let fresh_braces = with_shortcut("{}", &READ_ALOUD, SPEAK_CMD).unwrap();
        assert_eq!(
            status(&fresh_braces, &READ_ALOUD),
            Status::Installed {
                keys: "Super+Shift+Space".to_string()
            }
        );

        let existing = "{\n    (\n        modifiers: [\n            Ctrl,\n        ],\n        key: \"t\",\n        description: Some(\"Terminal\"),\n    ): Spawn(\"term\"),\n}";
        let merged = with_shortcut(existing, &DICTATE, RECORD_CMD).unwrap();
        assert!(merged.contains("Terminal"));
        assert_eq!(entries(&merged).len(), 2, "{merged}");
        // Super Voice's entry comes after the existing one, before the final close.
        assert!(merged.rfind("Super Voice").unwrap() > merged.find("Terminal").unwrap());
    }

    /// A last entry without its comma gets one, so the entry after it is
    /// still part of the map.
    #[test]
    fn add_puts_back_a_missing_comma() {
        let c = r#"{ (modifiers: [Ctrl], key: "t"): Spawn("term") }"#;
        let merged = with_shortcut(c, &READ_ALOUD, SPEAK_CMD).unwrap();
        assert!(merged.contains(r#"Spawn("term"),"#), "{merged}");
        assert_eq!(entries(&merged).len(), 2, "{merged}");
    }

    /// Removing a shortcut takes out its entry and comma, and nothing else.
    /// The other shortcut, named "Super Voice: read aloud", is not mistaken
    /// for dictation's "Super Voice".
    #[test]
    fn remove_takes_out_only_that_shortcut() {
        let both = with_shortcut(WITH_RECORD, &READ_ALOUD, SPEAK_CMD).unwrap();

        let no_dictate = without_shortcut(&both, &DICTATE).expect("removed");
        assert_eq!(status(&no_dictate, &DICTATE), Status::Missing);
        assert!(is_installed(&no_dictate, &READ_ALOUD), "{no_dictate}");
        assert_eq!(entries(&no_dictate).len(), 2, "{no_dictate}");
        assert!(no_dictate.contains("): Disable,"), "{no_dictate}");

        let neither = without_shortcut(&no_dictate, &READ_ALOUD).expect("removed");
        assert_eq!(status(&neither, &READ_ALOUD), Status::Missing);
        assert_eq!(entries(&neither).len(), 1, "{neither}");
        assert!(neither.trim_end().ends_with('}'), "{neither}");

        // Added back, it is in again.
        let again = with_shortcut(&neither, &READ_ALOUD, SPEAK_CMD).unwrap();
        assert!(is_installed(&again, &READ_ALOUD));

        assert!(without_shortcut(&neither, &DICTATE).is_none());
        assert!(without_shortcut("", &DICTATE).is_none());
    }

    /// A shortcut bound by hand to other keys, and a last entry with no
    /// comma, come out the same way.
    #[test]
    fn remove_takes_out_a_hand_bound_last_entry() {
        let c = r#"{ (modifiers: [Ctrl], key: "t"): Spawn("term"), (modifiers: [Ctrl, Alt], key: "r"): Spawn("voice speak --read") }"#;
        let out = without_shortcut(c, &READ_ALOUD).expect("removed");
        assert!(!is_installed(&out, &READ_ALOUD), "{out}");
        assert_eq!(entries(&out).len(), 1, "{out}");
        assert!(out.contains(r#"Spawn("term")"#), "{out}");
    }

    /// The keys a message names, as people write them.
    #[test]
    fn keys_read_as_people_write_them() {
        assert_eq!(DICTATE.keys(), "Super+Space");
        assert_eq!(READ_ALOUD.keys(), "Super+Shift+Space");
    }
}
