// SPDX-License-Identifier: GPL-3.0-only
//! `stt service`: register, unregister, or report on the app bundle's two
//! `LaunchAgent`s, the daemon and the shortcut listener.
//!
//! The settings app registers them the first time it is opened, which is all
//! someone who drags Super STT into Applications needs. This is for the rest:
//! the justfile's install and uninstall, and anyone stopping the daemon from
//! a terminal. It works only as the CLI inside the bundle — through `stt`, or
//! `Super STT.app/Contents/MacOS/super-stt-cli` — because `ServiceManagement`
//! finds the agents through the calling executable's bundle.

use anyhow::{Result, bail};
use clap::{ArgMatches, Command};
use super_stt_shared::launch_agents::{Agent, Status};

pub fn command() -> Command {
    Command::new("service")
        .about("Register or unregister the daemon and shortcut agents (macOS)")
        .subcommand_required(true)
        .subcommand(
            Command::new("register")
                .about("Register both agents, which starts them now and at every login"),
        )
        .subcommand(Command::new("unregister").about("Unregister both agents, which stops them"))
        .subcommand(Command::new("status").about("Print whether each agent is registered"))
}

/// # Errors
/// When an agent could not be registered or unregistered. The others are
/// still attempted, and each failure is printed.
pub fn run(sub: &ArgMatches) -> Result<()> {
    match sub.subcommand_name() {
        Some("register") => each(Agent::register),
        Some("unregister") => each(Agent::unregister),
        _ => {
            for agent in Agent::ALL {
                println!("{}: {}", agent.label(), describe(agent.status()));
            }
            Ok(())
        }
    }
}

fn each(action: fn(Agent) -> Result<()>) -> Result<()> {
    let mut failed = 0;
    for agent in Agent::ALL {
        match action(agent) {
            Ok(()) => println!("{}: {}", agent.label(), describe(agent.status())),
            Err(e) => {
                eprintln!("{e:#}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        bail!("{failed} of {} agents failed", Agent::ALL.len());
    }
    Ok(())
}

fn describe(status: Status) -> &'static str {
    match status {
        Status::Enabled => "registered",
        Status::NotRegistered => "not registered",
        Status::RequiresApproval => {
            "registered, but switched off in System Settings › General › Login Items"
        }
        Status::NotInBundle => "not in this executable's app bundle",
    }
}
