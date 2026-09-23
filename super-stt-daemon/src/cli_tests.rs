// SPDX-License-Identifier: GPL-3.0-only
//! The daemon's command line is parsed strictly (`get_matches` exits the
//! process on an unknown argument), so every `ExecStart` the repo ships or
//! generates has to be one this binary accepts.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate lives one level under the repo root")
        .to_path_buf()
}

/// Packaging files that either ship an `ExecStart=` line or rewrite one.
const PACKAGING: &[&str] = &["super-stt-daemon/systemd/super-stt.service", "justfile"];

/// Packaging files that declare the daemon's argv as a launchd
/// `ProgramArguments` array instead.
///
/// Listed separately from [`PACKAGING`] because the syntax is unrelated, not
/// because the rule is: a flag added here crash-loops the agent under
/// `KeepAlive` exactly as one added to `ExecStart=` crash-loops the unit
/// under `Restart=always`, and the daemon's clap surface is the same one on
/// both platforms.
const PACKAGING_PLISTS: &[&str] = &["super-stt-daemon/launchd/ai.menjivar.super-stt.plist"];

/// The `ProgramArguments` array of a launchd plist, as the argument tokens
/// that follow the program itself.
///
/// A deliberately small reader rather than a plist parser: the one thing it
/// must not do is silently find nothing and let the test pass, so it returns
/// the strings between `<array>` and `</array>` and the caller asserts it
/// found some.
fn plist_program_args(text: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for (offset, _) in text.match_indices("<key>ProgramArguments</key>") {
        let rest = &text[offset..];
        let Some(open) = rest.find("<array>") else {
            continue;
        };
        let Some(close) = rest.find("</array>") else {
            continue;
        };
        if close < open {
            continue;
        }
        let body = &rest[open + "<array>".len()..close];
        let mut tokens = Vec::new();
        let mut remainder = body;
        while let Some(start) = remainder.find("<string>") {
            let after = &remainder[start + "<string>".len()..];
            let Some(end) = after.find("</string>") else {
                break;
            };
            tokens.push(after[..end].trim().to_string());
            remainder = &after[end + "</string>".len()..];
        }
        if tokens.is_empty() {
            continue;
        }
        // Drop the program itself; the rest is argv.
        out.push(tokens.split_off(1));
    }
    out
}

/// Every `ExecStart=` occurrence in `text`, as the argument tokens that follow
/// the binary.
///
/// `just` templates (`{{ name }}`) and shell expansions (`$var`, `${var}`) are
/// substituted values, not literal argv — they collapse to a single opaque
/// token so a templated binary path does not read as an argument. The scan
/// stops at the delimiters that end an `ExecStart` in the formats used here:
/// end of line, and the `|` / quote characters that close a `sed`
/// replacement.
fn execstart_args(text: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for (offset, _) in text.match_indices("ExecStart=") {
        let rest = &text[offset + "ExecStart=".len()..];
        let end = rest.find(['|', '"', '\'', '\n']).unwrap_or(rest.len());
        let mut tokens = Vec::new();
        let mut chars = rest[..end].chars().peekable();
        let mut current = String::new();
        while let Some(c) = chars.next() {
            match c {
                // `{{ … }}` / `$…` stand in for a value substituted at run
                // time; keep them as one placeholder token.
                '{' if chars.peek() == Some(&'{') => {
                    while let Some(c) = chars.next() {
                        if c == '}' && chars.peek() == Some(&'}') {
                            chars.next();
                            break;
                        }
                    }
                    current.push_str("SUBST");
                }
                '$' => {
                    while chars
                        .peek()
                        .is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '{' | '}'))
                    {
                        chars.next();
                    }
                    current.push_str("SUBST");
                }
                c if c.is_whitespace() => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                }
                c => current.push(c),
            }
        }
        if !current.is_empty() {
            tokens.push(current);
        }
        if tokens.is_empty() {
            continue;
        }
        // Drop the binary itself; the rest is argv.
        out.push(tokens.split_off(1));
    }
    out
}

/// The daemon takes no configuration on its command line — the model, its
/// device, and the audio theme are all config / `POST /v1` state. Packaging
/// that bakes such a value into `ExecStart` does not merely get ignored: clap
/// rejects the unknown argument and exits 2 before the listener binds, and
/// `Restart=always` turns that into a permanent crash loop with no socket for
/// the app or the CLI to reach.
///
/// This is the test that fails if an installer starts writing a flag the
/// binary no longer accepts.
#[test]
fn every_shipped_execstart_parses() {
    let root = repo_root();
    let mut checked = 0;
    for rel in PACKAGING {
        let path = root.join(rel);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for args in execstart_args(&text) {
            let argv: Vec<String> = std::iter::once("super-stt-daemon".to_string())
                .chain(args.iter().cloned())
                .collect();
            assert!(
                super::build().try_get_matches_from(&argv).is_ok(),
                "{rel}: `ExecStart` passes {args:?}, which the daemon's clap surface rejects — \
                 the unit exits 2 at startup and crash-loops under Restart=always"
            );
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "no `ExecStart=` found in {PACKAGING:?} — this test would pass while validating nothing"
    );
}

/// The launchd half of [`every_shipped_execstart_parses`]. Same invariant,
/// same consequence, different file format — see [`PACKAGING_PLISTS`].
#[test]
fn every_shipped_program_arguments_parses() {
    let root = repo_root();
    let mut checked = 0;
    for rel in PACKAGING_PLISTS {
        let path = root.join(rel);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for args in plist_program_args(&text) {
            let argv: Vec<String> = std::iter::once("super-stt-daemon".to_string())
                .chain(args.iter().cloned())
                .collect();
            assert!(
                super::build().try_get_matches_from(&argv).is_ok(),
                "{rel}: `ProgramArguments` passes {args:?}, which the daemon's clap surface \
                 rejects — the agent exits 2 at startup and crash-loops under KeepAlive"
            );
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "no `ProgramArguments` found in {PACKAGING_PLISTS:?} — this test would pass while \
         validating nothing"
    );
}

/// The plist reader has to actually see an injected flag, or the test above
/// passes for the wrong reason.
#[test]
fn the_plist_scan_catches_an_injected_flag() {
    let plist = "<key>ProgramArguments</key>\n<array>\n\
         <string>/usr/local/bin/super-stt-daemon</string>\n\
         <string>--model</string>\n<string>whisper</string>\n</array>";
    let found = plist_program_args(plist);
    assert_eq!(
        found,
        vec![vec!["--model".to_string(), "whisper".to_string()]]
    );
    let argv = vec![
        "super-stt-daemon".to_string(),
        "--model".to_string(),
        "whisper".to_string(),
    ];
    assert!(
        super::build().try_get_matches_from(&argv).is_err(),
        "the daemon should reject --model; if it now accepts one, this guard is obsolete"
    );
}

/// The extractor has to actually see an injected flag, or the test above
/// passes for the wrong reason. Feed it the exact line that regressed.
#[test]
fn the_execstart_scan_catches_an_injected_flag() {
    let sed = "sudo sed -i \"s|^ExecStart={{ daemon_bin_name }}$|ExecStart={{ daemon_bin_name }} --model $model|\" unit";
    let found = execstart_args(sed);
    assert!(
        found
            .iter()
            .any(|args| args.contains(&"--model".to_string())),
        "the scan missed an injected --model: {found:?}"
    );
    for args in found {
        let argv: Vec<String> = std::iter::once("super-stt-daemon".to_string())
            .chain(args.iter().cloned())
            .collect();
        if args.contains(&"--model".to_string()) {
            assert!(
                super::build().try_get_matches_from(&argv).is_err(),
                "clap accepted --model; the crash-loop guard cannot fire"
            );
        }
    }
}

/// A bare `ExecStart` (what the packaged unit ships) must read as zero
/// arguments, templated binary path or not.
#[test]
fn a_bare_execstart_has_no_arguments() {
    let no_args: Vec<Vec<String>> = vec![vec![]];
    assert_eq!(execstart_args("ExecStart=super-stt-daemon\n"), no_args);
    assert_eq!(execstart_args("ExecStart={{ daemon_bin_name }}\n"), no_args);
    // `ExecStartPre=` is a different directive and must not be scanned.
    assert!(execstart_args("ExecStartPre=/bin/sh -c 'true'\n").is_empty());
}
