// SPDX-License-Identifier: GPL-3.0-only
//! Installer and self-updater for Super STT. One binary serves the curl
//! bootstrap (interactive), the app's in-app update (--non-interactive
//! --json-progress), and the escalated file-placement step (--root-phase).

// Phase 4 builds this crate's library-ish core (errors, progress, resolve,
// download, verify, stage, root_phase); nothing in `main` calls into it
// yet — CLI orchestration arrives in Phase 5. Until then everything below
// is only exercised by `#[cfg(test)]`, which `cargo clippy`/`cargo build`
// (no --tests) don't see as "used". Drop this once Phase 5 wires the
// modules into `main`.
#![allow(dead_code)]

mod download;
mod errors;
mod progress;
mod resolve;
mod root_phase;
mod stage;
mod verify;

fn main() {
    super_stt_forge::install_crypto_provider();
    println!("super-stt-install {}", env!("CARGO_PKG_VERSION"));
}
