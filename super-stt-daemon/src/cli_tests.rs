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
const PACKAGING: &[&str] = &[
    "super-stt-daemon/systemd/super-stt.service",
    "justfile",
    "scripts/install-stable.sh",
    "scripts/install-beta.sh",
];

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
