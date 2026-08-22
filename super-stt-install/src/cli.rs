// SPDX-License-Identifier: GPL-3.0-only
//! `super-stt-install`'s command line: the curl-bootstrap/interactive
//! surface, the app's `--non-interactive --json-progress` in-app update
//! call, and the hidden escalated `--root-phase` step.

use clap::{Arg, ArgAction, Command, builder::PossibleValuesParser, value_parser};
use std::path::PathBuf;

// Four independent flags (`non_interactive`, `json_progress`, `beta`,
// `dry_run`) is the CLI's actual, fixed surface — the binary's documented
// contract other tools (the app, Phase 7) parse against — not a state
// machine candidate.
#[allow(clippy::struct_excessive_bools)]
pub struct Cli {
    pub non_interactive: bool,
    pub json_progress: bool,
    pub version: Option<String>,
    pub beta: bool,
    pub components: Option<crate::stage::ComponentSelection>,
    pub dry_run: bool,
    pub root_phase: Option<PathBuf>,
}

#[must_use]
pub fn parse_cli() -> Cli {
    let m = Command::new("super-stt-install")
        .about("Install or update Super STT (daemon, CLI, app, COSMIC applet)")
        .disable_version_flag(true)
        .arg(
            Arg::new("non-interactive")
                .long("non-interactive")
                .action(ArgAction::SetTrue)
                .help("No prompts; auto-detect components on an existing install"),
        )
        .arg(
            Arg::new("json-progress")
                .long("json-progress")
                .action(ArgAction::SetTrue)
                .help("Emit machine-readable JSON progress events on stdout"),
        )
        .arg(
            Arg::new("version")
                .long("version")
                .value_name("TAG")
                .help("Pin a release tag (default: latest for the channel)"),
        )
        .arg(
            Arg::new("beta")
                .long("beta")
                .action(ArgAction::SetTrue)
                .help("Resolve against prereleases as well as stable releases"),
        )
        .arg(
            Arg::new("components")
                .long("components")
                .value_parser(PossibleValuesParser::new([
                    "all", "daemon", "app", "applet",
                ]))
                .help("Explicit component selection (default: detect)"),
        )
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .action(ArgAction::SetTrue)
                .help("Resolve, download, verify, and stage — but install nothing"),
        )
        .arg(
            Arg::new("root-phase")
                .long("root-phase")
                .value_name("MANIFEST")
                .value_parser(value_parser!(PathBuf))
                .hide(true),
        )
        .get_matches();
    Cli {
        non_interactive: m.get_flag("non-interactive"),
        json_progress: m.get_flag("json-progress"),
        version: m.get_one::<String>("version").cloned(),
        beta: m.get_flag("beta"),
        components: m.get_one::<String>("components").map(|s| s.as_str().into()),
        dry_run: m.get_flag("dry-run"),
        root_phase: m.get_one::<PathBuf>("root-phase").cloned(),
    }
}
