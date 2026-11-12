// SPDX-License-Identifier: GPL-3.0-only
//! GNOME's own bindings on Super Voice's keys.
//!
//! GNOME switches the input source on Super+Space, and back on
//! Super+Shift+Space, and its own binding gets a press before any app's
//! shortcut on the same keys: the portal binds Super Voice's, and it never
//! fires. So, as on COSMIC, where Dictate's entry takes Super+Space over,
//! the daemon takes a shortcut's keys from GNOME's input-source switching
//! before it binds the shortcut, and gives them back once it no longer binds
//! it. Input-source switching keeps its other keys, such as the keyboard's
//! own switch key.
//!
//! GNOME grabs an app's keys when the app binds them, so they are taken
//! first: freed after the bind, the press would still go nowhere.
//!
//! GNOME also keeps a record of each app's global shortcuts, which its
//! Settings lists on the app's page under Apps, and the portal cannot take
//! one out of it. So removing one, the daemon takes it out of the record
//! itself ([`forget`]), as Settings' own "Remove All Shortcuts" does for all
//! of them.
//!
//! GNOME's settings are read and written with `gsettings`, in the daemon's
//! own session, so in the user's settings for that session.

use super::portal::APP_ID;
use anyhow::{Context, Result, bail};
use log::debug;
use super_voice_shared::models::shortcuts::ShortcutId;

/// GNOME's keybindings.
const SCHEMA: &str = "org.gnome.desktop.wm.keybindings";

/// GNOME's record of one app's global shortcuts, under the app's id.
const APP_SHORTCUTS_SCHEMA: &str = "org.gnome.settings-daemon.global-shortcuts.application";

/// The GNOME binding that holds shortcut `id`'s keys, and those keys as GNOME
/// writes them.
fn held_by(id: ShortcutId) -> (&'static str, &'static str) {
    match id {
        ShortcutId::Dictate => ("switch-input-source", "<Super>space"),
        ShortcutId::ReadAloud => ("switch-input-source-backward", "<Shift><Super>space"),
    }
}

/// Take shortcut `id`'s keys from GNOME's input-source switching. Answers
/// whether they were taken: not when that binding was not on them.
///
/// # Errors
///
/// When `gsettings` cannot read or write the binding.
pub async fn take(id: ShortcutId) -> Result<bool> {
    let (key, keys) = held_by(id);
    let Some(rest) = without(&get(key).await?, keys) else {
        return Ok(false);
    };
    set(key, &rest).await?;
    debug!("GNOME: took {keys} from {key}");
    Ok(true)
}

/// Give shortcut `id`'s keys back to GNOME's input-source switching.
///
/// # Errors
///
/// When `gsettings` cannot read or write the binding.
pub async fn give_back(id: ShortcutId) -> Result<()> {
    let (key, keys) = held_by(id);
    if let Some(all) = with(&get(key).await?, keys) {
        set(key, &all).await?;
        debug!("GNOME: gave {keys} back to {key}");
    }
    Ok(())
}

/// Take shortcut `id` out of GNOME's record of Super Voice's shortcuts, so
/// it leaves GNOME's Settings, and adding it again asks again. Bind the rest
/// first: this only changes the record. Answers whether GNOME had it.
///
/// # Errors
///
/// When `gsettings` cannot read or write the record.
pub async fn forget(id: ShortcutId) -> Result<bool> {
    let schema =
        format!("{APP_SHORTCUTS_SCHEMA}:/org/gnome/settings-daemon/global-shortcuts/{APP_ID}/");
    let text = gsettings(&["get", &schema, "shortcuts"]).await?;
    let entries = parse_entries(&text)
        .with_context(|| format!("GNOME's record of the shortcuts is not as expected: {text}"))?;
    let rest: Vec<&str> = entries
        .iter()
        .filter(|(entry_id, _)| entry_id != id.as_wire_str())
        .map(|(_, entry)| entry.as_str())
        .collect();
    if rest.len() == entries.len() {
        return Ok(false);
    }
    if rest.is_empty() {
        gsettings(&["reset", &schema, "shortcuts"]).await?;
    } else {
        gsettings(&[
            "set",
            &schema,
            "shortcuts",
            &format!("[{}]", rest.join(", ")),
        ])
        .await?;
    }
    debug!("GNOME: forgot shortcut {id}");
    Ok(true)
}

/// The keys GNOME's binding `key` is on.
async fn get(key: &str) -> Result<Vec<String>> {
    let text = gsettings(&["get", SCHEMA, key]).await?;
    parse_list(&text).with_context(|| format!("{SCHEMA} {key} is not a list of keys: {text}"))
}

/// Put GNOME's binding `key` on `keys`.
async fn set(key: &str, keys: &[String]) -> Result<()> {
    gsettings(&["set", SCHEMA, key, &format_list(keys)]).await?;
    Ok(())
}

/// Run `gsettings` with `args`, and answer what it prints.
async fn gsettings(args: &[&str]) -> Result<String> {
    let out = tokio::process::Command::new("gsettings")
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .context("Could not run gsettings")?;
    if !out.status.success() {
        bail!(
            "gsettings {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The entries of GNOME's record of an app's shortcuts, as `gsettings get`
/// prints it, each with its id: `[('dictate', {'shortcuts': <['<Super>space']>,
/// 'description': <'Dictate'>}), …]`, or `@a(sa{sv}) []` when it is empty.
/// Each entry is kept as written, to be written back as it was.
fn parse_entries(text: &str) -> Option<Vec<(String, String)>> {
    let text = text.trim();
    // An empty list is printed with its type: `@a(sa{sv}) []`.
    let text = match text.strip_prefix('@') {
        Some(typed) => typed.split_once(' ')?.1.trim(),
        None => text,
    };
    let inner = text.strip_prefix('[')?.strip_suffix(']')?;
    let mut entries = Vec::new();
    let mut depth = 0_usize;
    let mut start = 0;
    let mut chars = inner.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            // A string is only ever inside an entry.
            '\'' | '"' if depth == 0 => return None,
            '\'' | '"' => {
                // A string: skip it, escapes and all.
                loop {
                    match chars.next()?.1 {
                        '\\' => {
                            chars.next()?;
                        }
                        end if end == c => break,
                        _ => {}
                    }
                }
            }
            '(' | '[' | '{' | '<' => {
                if depth == 0 {
                    if c != '(' {
                        return None;
                    }
                    start = at;
                }
                depth += 1;
            }
            ')' | ']' | '}' | '>' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    let entry = &inner[start..=at];
                    entries.push((first_string(entry)?, entry.to_string()));
                }
            }
            ' ' | ',' if depth == 0 => {}
            _ if depth == 0 => return None,
            _ => {}
        }
    }
    (depth == 0).then_some(entries)
}

/// The first string in `text`: an entry's id.
fn first_string(text: &str) -> Option<String> {
    let mut chars = text.chars().skip_while(|c| *c != '\'' && *c != '"');
    let quote = chars.next()?;
    let mut out = String::new();
    loop {
        match chars.next()? {
            '\\' => out.push(chars.next()?),
            c if c == quote => return Some(out),
            c => out.push(c),
        }
    }
}

/// A list of strings as `gsettings get` prints one: `['<Super>space',
/// 'XF86Keyboard']`, or `@as []` when it is empty.
fn parse_list(text: &str) -> Option<Vec<String>> {
    let text = text.trim();
    let text = text.strip_prefix("@as").unwrap_or(text).trim();
    let inner = text.strip_prefix('[')?.strip_suffix(']')?;
    let mut items = Vec::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | ',' => {}
            '\'' | '"' => {
                let mut item = String::new();
                loop {
                    match chars.next()? {
                        '\\' => item.push(chars.next()?),
                        end if end == c => break,
                        other => item.push(other),
                    }
                }
                items.push(item);
            }
            _ => return None,
        }
    }
    Some(items)
}

/// `keys` as `gsettings set` takes a list of strings.
fn format_list(keys: &[String]) -> String {
    let items: Vec<String> = keys
        .iter()
        .map(|k| format!("'{}'", k.replace('\\', "\\\\").replace('\'', "\\'")))
        .collect();
    format!("[{}]", items.join(", "))
}

/// Keys as GNOME writes them, in a form two ways of writing the same keys
/// share: the modifiers in order, lower-case, and the key, lower-case.
/// `<Shift><Super>space` and `<super><shift>Space` are the same keys.
fn normal(keys: &str) -> (Vec<String>, String) {
    let mut modifiers = Vec::new();
    let mut rest = keys;
    while let Some(after) = rest.strip_prefix('<') {
        let Some((modifier, tail)) = after.split_once('>') else {
            break;
        };
        modifiers.push(modifier.to_ascii_lowercase());
        rest = tail;
    }
    modifiers.sort();
    (modifiers, rest.to_ascii_lowercase())
}

/// `list` without `keys`, or `None` when it does not have them.
fn without(list: &[String], keys: &str) -> Option<Vec<String>> {
    let keys = normal(keys);
    let rest: Vec<String> = list.iter().filter(|k| normal(k) != keys).cloned().collect();
    (rest.len() < list.len()).then_some(rest)
}

/// `list` with `keys` first, where GNOME has them by default, or `None` when
/// it has them already.
fn with(list: &[String], keys: &str) -> Option<Vec<String>> {
    let normal_keys = normal(keys);
    if list.iter().any(|k| normal(k) == normal_keys) {
        return None;
    }
    Some(
        std::iter::once(keys.to_string())
            .chain(list.iter().cloned())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn a_list_reads_as_gsettings_prints_it() {
        assert_eq!(
            parse_list("['<Super>space', 'XF86Keyboard']\n"),
            Some(strings(&["<Super>space", "XF86Keyboard"]))
        );
        assert_eq!(parse_list("@as []\n"), Some(Vec::new()));
        assert_eq!(parse_list("[]"), Some(Vec::new()));
        assert_eq!(
            parse_list(r#"["it's", 'a\'b']"#),
            Some(strings(&["it's", "a'b"]))
        );
        assert_eq!(parse_list("nothing"), None);
        assert_eq!(parse_list("['unclosed]"), None);
    }

    #[test]
    fn a_list_is_written_as_gsettings_takes_it() {
        assert_eq!(
            format_list(&strings(&["<Super>space", "XF86Keyboard"])),
            "['<Super>space', 'XF86Keyboard']"
        );
        assert_eq!(format_list(&[]), "[]");
        let odd = strings(&["a'b", r"c\d"]);
        assert_eq!(parse_list(&format_list(&odd)), Some(odd));
    }

    /// The shortcut's keys leave input-source switching, which keeps the
    /// rest, however GNOME wrote them.
    #[test]
    fn taking_keys_keeps_the_others() {
        let defaults = strings(&["<Super>space", "XF86Keyboard"]);
        assert_eq!(
            without(&defaults, "<Super>space"),
            Some(strings(&["XF86Keyboard"]))
        );
        let backward = strings(&["<Super><Shift>Space", "<Shift>XF86Keyboard"]);
        assert_eq!(
            without(&backward, "<Shift><Super>space"),
            Some(strings(&["<Shift>XF86Keyboard"]))
        );
        assert_eq!(without(&strings(&["XF86Keyboard"]), "<Super>space"), None);
        assert_eq!(
            without(&strings(&["<Shift><Super>space"]), "<Super>space"),
            None
        );
    }

    /// Given back, the keys go first, where GNOME has them, and only when
    /// they are not there already.
    #[test]
    fn keys_given_back_go_first() {
        assert_eq!(
            with(&strings(&["XF86Keyboard"]), "<Super>space"),
            Some(strings(&["<Super>space", "XF86Keyboard"]))
        );
        assert_eq!(
            with(&[], "<Shift><Super>space"),
            Some(strings(&["<Shift><Super>space"]))
        );
        assert_eq!(
            with(&strings(&["<super>Space", "XF86Keyboard"]), "<Super>space"),
            None
        );
    }

    /// GNOME's record reads as its entries, each by its id and as written,
    /// whatever the keys hold.
    #[test]
    fn gnomes_record_reads_as_its_entries() {
        let dictate = "('dictate', {'shortcuts': <['<Super>space']>, 'description': <'Dictate'>})";
        let read = "('read_aloud', {'shortcuts': <['<Shift><Super>space']>, 'description': <\"Read 'aloud'\">})";
        let entries = parse_entries(&format!("[{dictate}, {read}]\n")).expect("parses");
        assert_eq!(
            entries,
            [
                ("dictate".to_string(), dictate.to_string()),
                ("read_aloud".to_string(), read.to_string())
            ]
        );
        assert_eq!(parse_entries("@a(sa{sv}) []\n"), Some(Vec::new()));
        assert_eq!(parse_entries("[]"), Some(Vec::new()));
        assert_eq!(parse_entries("[('dictate', {}"), None);
        assert_eq!(parse_entries("['dictate']"), None);
        assert_eq!(parse_entries("nothing"), None);
    }

    #[test]
    fn each_shortcut_is_on_its_input_source_binding() {
        assert_eq!(
            held_by(ShortcutId::Dictate),
            ("switch-input-source", "<Super>space")
        );
        assert_eq!(
            held_by(ShortcutId::ReadAloud),
            ("switch-input-source-backward", "<Shift><Super>space")
        );
    }
}
